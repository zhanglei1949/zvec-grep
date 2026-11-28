//! Unit and multi-policy ablation tests for bounded pattern-constrained graph search.

use std::path::{Path, PathBuf};

use crate::{
    api::context::{
        options::{
            ContextRoute, ContextRouteMode, GraphRankingPolicy, GraphSearchMode,
            GraphSearchOptions, QueryFilter,
        },
        result::MatchedBy,
    },
    domain::{
        CodeMetadata, Content, Direction, EdgeProvenance, Entity, EntityFragment, EntityId,
        EntityMetadata, FileEdge, FileGraphNode, FileGraphResult, FileId, FileIndexStatus,
        FileRecord, FileSnapshot, FragmentId, GraphEdgeKind, PendingRef, PendingRefStatus, Range,
        SourcePath, SymbolType,
        model::{EmbeddingMetric, EmbeddingModelInfo, ModelInfo},
    },
    pipelines::indexed_search::{
        pipeline::{SearchPlan, search_workspace_index},
        storage::SearchStorage,
    },
    storage::{
        IndexStore,
        graph::Resolution,
        types::{IndexedFragment, WorkspaceIndexStorageOptions},
    },
    utils::{line_byte_offsets, text_range_from_offsets},
};

fn test_model_info() -> EmbeddingModelInfo {
    EmbeddingModelInfo::new(
        ModelInfo::new(
            "local".to_owned(),
            "graph-test".to_owned(),
            [crate::domain::ContentKind::Code],
            8,
        )
        .expect("model info"),
        2,
        EmbeddingMetric::Cosine,
        None,
        None,
    )
    .expect("embedding info")
}

fn allocate_file(store: &IndexStore, path: &str, text: &str) -> FileRecord {
    let ids = store
        .resolve_file_ids(&[PathBuf::from(path)])
        .expect("resolve file id");
    FileRecord::new(
        ids[0],
        SourcePath::new(path).expect("source path"),
        FileSnapshot {
            size_bytes: text.len() as u64,
            modified_epoch_ms: Some(1_000),
            content_hash: Some(crate::utils::sha256_hex(text.as_bytes())),
        },
        FileIndexStatus::NotIndexed,
    )
    .expect("file record")
}

fn make_code_entity(
    file_id: FileId,
    file_text: &str,
    slice: &str,
    name: &str,
    scope: Option<&str>,
    kind: SymbolType,
) -> (Entity, IndexedFragment) {
    let start = file_text.find(slice).expect("slice present in file_text");
    let end = start + slice.len();
    let range = Range::Text(
        text_range_from_offsets(file_text, &line_byte_offsets(file_text), start, end)
            .expect("valid text range"),
    );
    let content = Content::Code(slice.to_owned());
    let id = EntityId::new(file_id, &content, &range).expect("entity id");
    let frag_id = FragmentId::new(&id, 0);
    let fragment = EntityFragment {
        id: frag_id.clone(),
        range: Range::Full,
    };
    let meta = CodeMetadata {
        symbol_type: Some(kind),
        symbol_name: Some(name.to_owned()),
        scope: scope.map(str::to_owned),
        ..CodeMetadata::default()
    };
    let entity = Entity::new(
        id.clone(),
        file_id,
        range,
        content,
        Some(EntityMetadata::Code(meta)),
        vec![fragment],
    )
    .expect("entity");
    let indexed = IndexedFragment {
        entity_id: id,
        fragment_id: frag_id,
        model: "local/graph-test".to_owned(),
        vector: vec![1.0, 0.0],
        fts_text: format!("{name} {slice}"),
    };
    (entity, indexed)
}

fn graph_node(entity: &Entity) -> FileGraphNode {
    let (kind, name, scope) = match entity.metadata() {
        Some(EntityMetadata::Code(c)) => (
            c.symbol_type.unwrap_or(SymbolType::Function),
            c.symbol_name.clone(),
            c.scope.clone(),
        ),
        _ => (SymbolType::Function, None, None),
    };
    let qname = match (scope.as_deref(), name.as_deref()) {
        (Some(s), Some(n)) => format!("{s}::{n}"),
        (None, Some(n)) => n.to_owned(),
        _ => entity.id().as_str().to_owned(),
    };
    FileGraphNode {
        id: entity.id().as_str().to_owned(),
        kind,
        name,
        qualified_name: qname,
        language: "python".to_owned(),
        range: None,
        signature: None,
        doc: None,
        arity: None,
        visibility: None,
        is_exported: true,
        is_declaration: false,
    }
}

fn resolved_edge(
    kind: GraphEdgeKind,
    src: &Entity,
    dst: &Entity,
    line: usize,
    provenance: EdgeProvenance,
) -> FileEdge {
    FileEdge {
        kind,
        source: src.id().as_str().to_owned(),
        target: dst.id().as_str().to_owned(),
        line: Some(line),
        column: Some(0),
        provenance,
        metadata: Default::default(),
    }
}

fn open_test_store(dir: &Path) -> IndexStore {
    IndexStore::open(WorkspaceIndexStorageOptions::ReadWrite {
        storage_path: dir.to_path_buf(),
        graph_enabled: true,
        embeddings: vec![test_model_info()],
    })
    .expect("open index store")
}

fn pending_ref(
    kind: GraphEdgeKind,
    owner: &Entity,
    ref_name: &str,
    line: usize,
) -> PendingRef {
    PendingRef {
        owner_id: owner.id().as_str().to_owned(),
        ref_name: ref_name.to_owned(),
        receiver_name: None,
        ref_kind: kind,
        arity: None,
        line,
        column: 0,
        status: PendingRefStatus::Pending,
        metadata: Default::default(),
    }
}

fn write_file_with_graph(
    store: &IndexStore,
    file: &FileRecord,
    entities: &[Entity],
    fragments: &[IndexedFragment],
    edges: Vec<FileEdge>,
) {
    write_file_with_graph_and_refs(store, file, entities, fragments, edges, Vec::new());
}

fn write_file_with_graph_and_refs(
    store: &IndexStore,
    file: &FileRecord,
    entities: &[Entity],
    fragments: &[IndexedFragment],
    edges: Vec<FileEdge>,
    pending_refs: Vec<PendingRef>,
) {
    let graph = FileGraphResult {
        nodes: entities.iter().map(graph_node).collect(),
        edges,
        pending_refs,
        directives: Vec::new(),
    };
    store
        .replace_file_with_graph(file, entities, fragments, &graph)
        .expect("replace file with graph");
}

fn resolve_all_pending(store: &IndexStore, targets: &[(&str, &Entity, EdgeProvenance)]) {
    let snapshot = store
        .resolution_snapshot(None)
        .expect("resolution snapshot");
    let page = store
        .list_pending_refs(100, 0, &snapshot)
        .expect("list pending refs");
    let resolutions: Vec<Resolution> = page
        .refs
        .iter()
        .filter_map(|stored| {
            let (_, target, provenance) = targets
                .iter()
                .find(|(name, _, _)| *name == stored.reference.ref_name)?;
            Some(Resolution {
                ref_id: stored.id,
                target_id: target.id().as_str().to_owned(),
                provenance: *provenance,
            })
        })
        .collect();
    assert_eq!(resolutions.len(), page.refs.len(), "all test refs resolved");
    store
        .apply_resolutions(&resolutions, &snapshot)
        .expect("apply resolutions");
}

#[tokio::test]
async fn p1_p2_and_p3_backfill_out_of_pool_targets_while_blocking_tests_and_zigzags() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = open_test_store(dir.path());

    // File 1: Seed caller `setActiveCell` -> P1 callee `update_row_index` -> P2 callee `persist_cursor`
    let text1 = "def setActiveCell(ref):\n    update_row_index(ref)\n";
    let f1 = allocate_file(&store, "src/sheet.py", text1);
    let (e_seed, frag_seed) = make_code_entity(
        f1.id(),
        text1,
        text1,
        "setActiveCell",
        Some("Sheet"),
        SymbolType::Function,
    );

    // File 2: Out-of-pool P1 target (zero lexical overlap with query!)
    let text2 = "def update_row_index(r):\n    persist_cursor(r)\n";
    let f2 = allocate_file(&store, "src/selection_record.py", text2);
    let (e_p1, frag_p1) = make_code_entity(
        f2.id(),
        text2,
        text2,
        "update_row_index",
        Some("SelectionRecord"),
        SymbolType::Function,
    );

    // File 3: Out-of-pool P2 monotonic 2-hop target (Out -> Out)
    let text3 = "def persist_cursor(r):\n    return r\n";
    let f3 = allocate_file(&store, "src/cursor_store.py", text3);
    let (e_p2, frag_p2) = make_code_entity(
        f3.id(),
        text3,
        text3,
        "persist_cursor",
        Some("CursorStore"),
        SymbolType::Function,
    );

    // File 4: Zig-zag distractor `unrelated_caller -> update_row_index` (Out -> In must NOT be traversed!)
    let text4 = "def unrelated_caller():\n    update_row_index(0)\n";
    let f4 = allocate_file(&store, "src/unrelated.py", text4);
    let (e_zigzag, frag_zigzag) = make_code_entity(
        f4.id(),
        text4,
        text4,
        "unrelated_caller",
        None,
        SymbolType::Function,
    );

    // File 5: Test file connected directly to seed (must be blocked by test-file barrier!)
    let text5 = "def test_active():\n    return 1\n";
    let f5 = allocate_file(&store, "tests/test_sheet.py", text5);
    let (e_test, frag_test) = make_code_entity(
        f5.id(),
        text5,
        text5,
        "test_active",
        None,
        SymbolType::Function,
    );

    write_file_with_graph_and_refs(
        &store,
        &f1,
        std::slice::from_ref(&e_seed),
        &[frag_seed],
        vec![],
        vec![pending_ref(
            GraphEdgeKind::Calls,
            &e_seed,
            "update_row_index",
            2,
        )],
    );
    write_file_with_graph_and_refs(
        &store,
        &f2,
        std::slice::from_ref(&e_p1),
        &[frag_p1],
        vec![],
        vec![pending_ref(GraphEdgeKind::Calls, &e_p1, "persist_cursor", 2)],
    );
    write_file_with_graph(&store, &f3, std::slice::from_ref(&e_p2), &[frag_p2], vec![]);
    write_file_with_graph_and_refs(
        &store,
        &f4,
        std::slice::from_ref(&e_zigzag),
        &[frag_zigzag],
        vec![],
        vec![pending_ref(
            GraphEdgeKind::Calls,
            &e_zigzag,
            "update_row_index",
            2,
        )],
    );
    write_file_with_graph_and_refs(
        &store,
        &f5,
        std::slice::from_ref(&e_test),
        &[frag_test],
        vec![],
        vec![pending_ref(
            GraphEdgeKind::Calls,
            &e_test,
            "setActiveCell",
            2,
        )],
    );
    resolve_all_pending(
        &store,
        &[
            ("update_row_index", &e_p1, EdgeProvenance::ImportBound),
            ("persist_cursor", &e_p2, EdgeProvenance::FileScoped),
            ("setActiveCell", &e_seed, EdgeProvenance::ImportBound),
        ],
    );

    // Files 6..=12: Low-score tail fillers so K=8 has 8 hits and quota=2 tail slots.
    for idx in 6_u32..=12 {
        let path = format!("src/filler_{idx}.py");
        let body = format!(
            "def filler_{idx}():\n    # setActiveCell mention in comment\n    return {idx}\n"
        );
        let f = allocate_file(&store, &path, &body);
        let (e, frag) = make_code_entity(
            f.id(),
            &body,
            &body,
            &format!("filler_{idx}"),
            None,
            SymbolType::Function,
        );
        write_file_with_graph(&store, &f, &[e], &[frag], vec![]);
    }

    let make_plan = |policy: GraphRankingPolicy, mode: GraphSearchMode| SearchPlan {
        routes: vec![ContextRoute {
            mode: ContextRouteMode::Fts,
            query: "setActiveCell".to_owned(),
        }],
        limit: Some(8),
        trace: true,
        prefer_symbol: false,
        filter: QueryFilter::default(),
        graph_search: GraphSearchOptions {
            mode,
            policy,
            ..GraphSearchOptions::default()
        },
    };

    // Baseline Off: neither out-of-pool P1 nor P2 is returned.
    let off = search_workspace_index(
        dir.path(),
        make_plan(GraphRankingPolicy::PatternTwoTrack, GraphSearchMode::Off),
        &store as &dyn SearchStorage,
        &[],
    )
    .await
    .expect("off search");
    assert_eq!(off.hits[0].entity.id(), e_seed.id());
    assert!(
        !off.hits.iter().any(|h| h.entity.id() == e_p1.id()),
        "off hits unexpectedly contained e_p1: {:?}",
        off.hits
            .iter()
            .map(|h| (h.rank, h.file.relative_path().display().to_string(), h.score))
            .collect::<Vec<_>>()
    );
    assert!(!off.hits.iter().any(|h| h.entity.id() == e_p2.id()));

    // InPoolRerankOnly: zero backfill, anchor stays #1.
    let in_pool = search_workspace_index(
        dir.path(),
        make_plan(GraphRankingPolicy::InPoolRerankOnly, GraphSearchMode::Auto),
        &store as &dyn SearchStorage,
        &[],
    )
    .await
    .expect("in-pool search");
    assert_eq!(in_pool.hits[0].entity.id(), e_seed.id());
    assert_eq!(
        in_pool
            .graph_diagnostics
            .as_ref()
            .expect("graph diag")
            .backfilled_count,
        0
    );

    // PatternTwoTrack: anchor stays #1, P1 & P2 backfill into tail slots, zig-zag blocked!
    let two_track = search_workspace_index(
        dir.path(),
        make_plan(GraphRankingPolicy::PatternTwoTrack, GraphSearchMode::Auto),
        &store as &dyn SearchStorage,
        &[],
    )
    .await
    .expect("two-track search");
    assert_eq!(two_track.hits[0].entity.id(), e_seed.id());
    assert!(
        two_track.hits.iter().any(|h| h.entity.id() == e_p1.id()),
        "P1 1-hop callee should backfill into results"
    );
    assert!(
        two_track.hits.iter().any(|h| h.entity.id() == e_p2.id()),
        "P2 monotonic 2-hop callee should backfill into results"
    );
    assert!(
        !two_track.hits.iter().any(|h| h.entity.id() == e_zigzag.id()),
        "zig-zag Out->In neighbor must never be traversed"
    );
    let p1_hit = two_track
        .hits
        .iter()
        .find(|h| h.entity.id() == e_p1.id())
        .expect("p1 hit");
    assert_eq!(p1_hit.matched_by, MatchedBy::Graph);
    let ev = p1_hit.graph_evidence.as_ref().expect("graph evidence");
    assert!(ev.backfilled);
    assert_eq!(ev.pattern, "P1");
    assert_eq!(ev.hops.len(), 1);
    assert_eq!(ev.hops[0].direction, Direction::Out);

    // LocalRwr ablation runs cleanly and surfaces the structurally reachable targets.
    let rwr = search_workspace_index(
        dir.path(),
        make_plan(GraphRankingPolicy::LocalRwr, GraphSearchMode::Auto),
        &store as &dyn SearchStorage,
        &[],
    )
    .await
    .expect("rwr search");
    assert_eq!(rwr.hits[0].entity.id(), e_seed.id());
    assert!(rwr.hits.iter().any(|h| h.entity.id() == e_p1.id()));
}

#[tokio::test]
async fn p3_polymorphic_override_recalls_only_matching_subclass_member() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = open_test_store(dir.path());

    // Base class Operation with method `reversible`
    let base_method = "    def reversible(self): return True\n";
    let base_text = format!("class Operation:\n{base_method}");
    let f1 = allocate_file(&store, "src/base.py", &base_text);
    let (e_base_cls, frag_base_cls) = make_code_entity(
        f1.id(),
        &base_text,
        &base_text,
        "Operation",
        None,
        SymbolType::Class,
    );
    let (e_base_m, frag_base_m) = make_code_entity(
        f1.id(),
        &base_text,
        base_method,
        "reversible",
        Some("Operation"),
        SymbolType::Function,
    );

    // Subclass RunPython with matching `reversible` AND unrelated `describe` (neither mentions Operation)
    let sub_rev = "    def reversible(self): return self.rev is not None\n";
    let sub_desc = "    def describe(self): return 'py'\n";
    let sub_text = format!("class RunPython:\n{sub_rev}{sub_desc}");
    let f2 = allocate_file(&store, "src/special.py", &sub_text);
    let (e_sub_cls, frag_sub_cls) = make_code_entity(
        f2.id(),
        &sub_text,
        &sub_text,
        "RunPython",
        None,
        SymbolType::Class,
    );
    let (e_sub_rev, frag_sub_rev) = make_code_entity(
        f2.id(),
        &sub_text,
        sub_rev,
        "reversible",
        Some("RunPython"),
        SymbolType::Function,
    );
    let (e_sub_desc, frag_sub_desc) = make_code_entity(
        f2.id(),
        &sub_text,
        sub_desc,
        "describe",
        Some("RunPython"),
        SymbolType::Function,
    );

    write_file_with_graph(
        &store,
        &f1,
        &[e_base_cls.clone(), e_base_m.clone()],
        &[frag_base_cls, frag_base_m],
        vec![resolved_edge(
            GraphEdgeKind::Contains,
            &e_base_cls,
            &e_base_m,
            2,
            EdgeProvenance::FileLocal,
        )],
    );
    write_file_with_graph_and_refs(
        &store,
        &f2,
        &[e_sub_cls.clone(), e_sub_rev.clone(), e_sub_desc.clone()],
        &[frag_sub_cls, frag_sub_rev, frag_sub_desc],
        vec![
            resolved_edge(
                GraphEdgeKind::Contains,
                &e_sub_cls,
                &e_sub_rev,
                2,
                EdgeProvenance::FileLocal,
            ),
            resolved_edge(
                GraphEdgeKind::Contains,
                &e_sub_cls,
                &e_sub_desc,
                3,
                EdgeProvenance::FileLocal,
            ),
        ],
        vec![pending_ref(GraphEdgeKind::Extends, &e_sub_cls, "Operation", 1)],
    );
    resolve_all_pending(
        &store,
        &[("Operation", &e_base_cls, EdgeProvenance::ImportBound)],
    );

    // Add 4 filler files so K=6 has tail slots available.
    for idx in 3_u32..=6 {
        let body = format!("def other_{idx}():\n    # Operation mention\n    return {idx}\n");
        let f = allocate_file(&store, &format!("src/other_{idx}.py"), &body);
        let (e, frag) = make_code_entity(
            f.id(),
            &body,
            &body,
            &format!("other_{idx}"),
            None,
            SymbolType::Function,
        );
        write_file_with_graph(&store, &f, &[e], &[frag], vec![]);
    }

    let res = search_workspace_index(
        dir.path(),
        SearchPlan {
            routes: vec![ContextRoute {
                mode: ContextRouteMode::Fts,
                query: "Operation.reversible".to_owned(),
            }],
            limit: Some(6),
            trace: false,
            prefer_symbol: false,
            filter: QueryFilter::default(),
            graph_search: GraphSearchOptions::default(),
        },
        &store as &dyn SearchStorage,
        &[],
    )
    .await
    .expect("p3 search");

    assert!(
        res.hits.iter().any(|h| h.entity.id() == e_sub_rev.id()),
        "P3 should recall subclass override RunPython.reversible"
    );
    assert!(
        !res.hits.iter().any(|h| h.entity.id() == e_sub_desc.id()),
        "P3 must NOT expand unrelated subclass member RunPython.describe"
    );
}

#[tokio::test]
async fn containment_dedup_frees_slot_when_parent_class_already_selected() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = open_test_store(dir.path());

    let method_body = "    def select_segments(self):\n        return self.items\n";
    let class_body = format!("class Segments:\n{method_body}    def extra(self): pass\n");
    let f1 = allocate_file(&store, "src/segments.py", &class_body);
    let (e_cls, frag_cls) = make_code_entity(
        f1.id(),
        &class_body,
        &class_body,
        "Segments",
        None,
        SymbolType::Class,
    );
    let (e_method, frag_method) = make_code_entity(
        f1.id(),
        &class_body,
        method_body,
        "select_segments",
        Some("Segments"),
        SymbolType::Function,
    );

    let other_body = "def handle_segments():\n    return Segments()\n";
    let f2 = allocate_file(&store, "src/other.py", other_body);
    let (e_other, frag_other) = make_code_entity(
        f2.id(),
        other_body,
        other_body,
        "handle_segments",
        None,
        SymbolType::Function,
    );

    write_file_with_graph(
        &store,
        &f1,
        &[e_cls.clone(), e_method.clone()],
        &[frag_cls, frag_method],
        vec![resolved_edge(
            GraphEdgeKind::Contains,
            &e_cls,
            &e_method,
            2,
            EdgeProvenance::FileLocal,
        )],
    );
    write_file_with_graph(
        &store,
        &f2,
        std::slice::from_ref(&e_other),
        &[frag_other],
        vec![],
    );

    let res = search_workspace_index(
        dir.path(),
        SearchPlan {
            routes: vec![ContextRoute {
                mode: ContextRouteMode::Fts,
                query: "Segments".to_owned(),
            }],
            limit: Some(2),
            trace: false,
            prefer_symbol: false,
            filter: QueryFilter::default(),
            graph_search: GraphSearchOptions::default(),
        },
        &store as &dyn SearchStorage,
        &[],
    )
    .await
    .expect("dedup search");

    let ids: Vec<_> = res.hits.iter().map(|h| h.entity.id()).collect();
    assert_eq!(ids, vec![e_cls.id(), e_other.id()]);
    assert!(
        res.graph_diagnostics
            .as_ref()
            .expect("diag")
            .containment_deduped
            >= 1
    );
}

#[tokio::test]
async fn same_file_call_cluster_receives_decay_relief_and_boost() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = open_test_store(dir.path());

    // Single file with an entry function calling a same-file helper (Quarry / SWE same-file cluster).
    let entry_src = "def dispatch_frame(pkt):\n    return decode_payload(pkt)\n";
    let helper_src = "def decode_payload(pkt):\n    # frame decoding helper\n    return pkt.body\n";
    let cluster_text = format!("{entry_src}\n{helper_src}");
    let f1 = allocate_file(&store, "src/wire.py", &cluster_text);
    let (e_entry, frag_entry) = make_code_entity(
        f1.id(),
        &cluster_text,
        entry_src,
        "dispatch_frame",
        None,
        SymbolType::Function,
    );
    let (e_helper, frag_helper) = make_code_entity(
        f1.id(),
        &cluster_text,
        helper_src,
        "decode_payload",
        None,
        SymbolType::Function,
    );

    // Another file with a distractor mentioning `dispatch_frame` in a comment.
    let dist_text = "def unrelated_log():\n    # dispatch_frame logging\n    return None\n";
    let f2 = allocate_file(&store, "src/log.py", dist_text);
    let (e_dist, frag_dist) = make_code_entity(
        f2.id(),
        dist_text,
        dist_text,
        "unrelated_log",
        None,
        SymbolType::Function,
    );

    write_file_with_graph(
        &store,
        &f1,
        &[e_entry.clone(), e_helper.clone()],
        &[frag_entry, frag_helper],
        vec![resolved_edge(
            GraphEdgeKind::Calls,
            &e_entry,
            &e_helper,
            2,
            EdgeProvenance::FileLocal,
        )],
    );
    write_file_with_graph(
        &store,
        &f2,
        std::slice::from_ref(&e_dist),
        &[frag_dist],
        vec![],
    );

    let res = search_workspace_index(
        dir.path(),
        SearchPlan {
            routes: vec![ContextRoute {
                mode: ContextRouteMode::Fts,
                query: "dispatch_frame".to_owned(),
            }],
            limit: Some(2),
            trace: true,
            prefer_symbol: false,
            filter: QueryFilter::default(),
            graph_search: GraphSearchOptions::default(),
        },
        &store as &dyn SearchStorage,
        &[],
    )
    .await
    .expect("cluster search");

    assert_eq!(res.hits[0].entity.id(), e_entry.id());
    assert_eq!(
        res.hits[1].entity.id(),
        e_helper.id(),
        "graph-linked same-file callee should outrank unlinked cross-file comment distractor"
    );
    let helper_trace = res.hits[1]
        .trace
        .as_ref()
        .expect("trace")
        .ranking
        .as_ref()
        .expect("ranking trace");
    assert!(
        helper_trace.diversity_factor > 0.75,
        "expected relieved decay factor ~0.833 (1/(1+0.2)), got {}",
        helper_trace.diversity_factor
    );
}
