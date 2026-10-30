use super::*;
use crate::graph::QueryStopReason;
use crate::relations::{bound::*, *};
use ::graphdb::params;
use std::time::Instant;

fn context() -> ProcessingContext {
    ProcessingContext {
        max_input_bytes: 64 << 20,
        max_output_items: 30_000_000,
        deadline: None,
        cancellation: tokio_util::sync::CancellationToken::new(),
    }
}

#[test]
fn readonly_sessions_share_owners_and_quarantine_but_not_cancel_controls()
-> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("graph");
    let store = GraphDbStore::create(&path, GraphLimits::default())?;
    let facts = fixture()?;
    store.write_facts(&facts, &context())?;
    let cancel = QueryControl::new()?;
    cancel.request(QueryStopReason::Cancelled);
    let cancelled = store.read_session(GraphLimits {
        control: Some(cancel),
        ..Default::default()
    })?;
    assert_eq!(
        cancelled
            .read_facts(&context())
            .expect_err("native cancellation")
            .kind,
        GraphErrorKind::Cancelled
    );
    let fresh = store.read_session(GraphLimits::default())?;
    assert_eq!(fresh.read_facts(&context())?, facts);
    assert_eq!(
        fresh
            .write_facts(&facts, &context())
            .expect_err("read session cannot write")
            .kind,
        GraphErrorKind::ReadOnly
    );
    // Adaptation-level error injection: native commit fault windows have their
    // own probes. This verifies quarantine propagation between owner facades.
    let unknown = GraphError {
        outcome: Some(GraphOutcome::Unknown),
        ..GraphError::local(GraphErrorKind::Storage)
    };
    assert!(store.finish::<()>(Err(unknown)).is_err());
    assert_eq!(
        fresh.counts().expect_err("shared quarantine").kind,
        GraphErrorKind::RecoveryRequired
    );
    drop(cancelled);
    drop(store);
    assert!(
        GraphDbStore::open(&path, false, GraphLimits::default()).is_err(),
        "live read facade retains native owner"
    );
    drop(fresh);
    GraphDbStore::open(&path, false, GraphLimits::default())?.read_facts(&context())?;
    Ok(())
}
fn version(path: &str, text: &str) -> SourceVersion {
    let digest = zg_storage::sha256_hex(text.as_bytes());
    SourceVersion {
        generation: "g2.1-fixture".into(),
        path: path.into(),
        raw_digest: digest.clone(),
        decoded_digest: digest,
    }
}
fn bind(mut facts: FileFacts, text: &str, id: u32) -> crate::Result<BoundFileFacts> {
    facts.validate(text, &context())?;
    for n in &mut facts.nodes {
        n.entity_indices = vec![0];
    }
    for r in &mut facts.references {
        r.entity_indices = vec![0];
    }
    let record = FileRecord {
        path: facts.version.path.clone(),
        file_id: id,
        index_status: IndexStatus::Indexed,
        content_hash: Some(facts.version.raw_digest.clone()),
        size_bytes: text.len() as u64,
        text_revision: u64::MAX,
        storage_generation: facts.version.generation.clone(),
        entity_count: 0,
        entity_ids_digest: String::new(),
    };
    BoundFileFacts::bind(facts, record, &[format!("{id:08x}{:024x}", 1)], &context())
}
#[expect(
    clippy::too_many_lines,
    reason = "one fixture covers all eight tables and nullable/tagged fields"
)]
fn fixture() -> crate::Result<FactCatalog> {
    let text = "helper()\n";
    let module = Some(CodeModule {
        namespace: "fixture".into(),
        path: vec!["a".into()],
        is_package: false,
    });
    let mut code = FileFacts::new(version("a.py", text), Language::Python, module, text)?;
    let id = node_id("a.py", Language::Python, "function", &["helper".into()], 0)?;
    code.nodes.push(FactNode {
        id: id.clone(),
        kind: NodeKind::Symbol,
        entity_kind: crate::relations::EntityKind::Function,
        name: "helper".into(),
        qualified_name: vec!["helper".into()],
        parent: Some(code.nodes[0].id.clone()),
        path_addressable: true,
        callable: true,
        span: Span {
            start: 0,
            end: text.len(),
        },
        entity_indices: vec![],
    });
    code.scopes.push(Scope {
        parent: Some(0),
        owner: id.clone(),
        kind: ScopeKind::Function,
    });
    code.bindings.extend([
        Binding {
            scope: 0,
            name: "helper".into(),
            target: BindingTarget::Node { id },
            visible_from: 0,
            visibility: String::new(),
        },
        Binding {
            scope: 1,
            name: "other".into(),
            target: BindingTarget::Import {
                path: vec!["quoted\"|中".into()],
            },
            visible_from: 3,
            visibility: String::new(),
        },
        Binding {
            scope: 1,
            name: "dynamic".into(),
            target: BindingTarget::Dynamic,
            visible_from: 0,
            visibility: String::new(),
        },
    ]);
    code.add_reference(
        1,
        RelationKind::Call,
        Span { start: 0, end: 6 },
        Lookup::Name {
            parts: vec!["helper".into()],
        },
    )?;
    code.diagnostics.extend([
        FactDiagnostic {
            code: "no_span".into(),
            span: None,
        },
        FactDiagnostic {
            code: "empty_span".into(),
            span: Some(Span { start: 0, end: 0 }),
        },
    ]);
    let md = "# 重复\r\n[x](b.md#same) [y](native.cpp) [z](missing.md)\n";
    let target = "# Same\n# Same\n";
    let mut files = vec![
        bind(code, text, 1)?,
        bind(
            extract_markdown(version("a.md", md), md, &context())?,
            md,
            2,
        )?,
        bind(
            extract_markdown(version("b.md", target), target, &context())?,
            target,
            3,
        )?,
    ];
    files.push(BoundFileFacts::inventory_only(
        FileRecord {
            path: "native.cpp".into(),
            file_id: 4,
            index_status: IndexStatus::Indexed,
            content_hash: Some(zg_storage::sha256_hex(b"c++")),
            size_bytes: u64::MAX,
            text_revision: u64::MAX,
            storage_generation: "g2.1-fixture".into(),
            entity_count: 0,
            entity_ids_digest: entity_ids_digest(&[])?,
        },
        &context(),
    )?);
    FactCatalog::resolve(
        files,
        stable_digest(&"config")?,
        stable_digest(&"selection")?,
        &context(),
    )
}

fn build_fixture() -> crate::Result<(FactCatalog, super::super::build::BuildSpec)> {
    use super::super::build::{BuildScope, BuildSpec};
    let configuration = serde_json::json!({"scope":"both","roots":[]});
    let policy_fingerprint = stable_digest(&configuration)?;
    let original = fixture()?;
    let catalog = FactCatalog::resolve(
        original.files,
        policy_fingerprint.clone(),
        original.selection_fingerprint,
        &context(),
    )?;
    let spec = BuildSpec {
        graph_build_id: "11111111-1111-4111-8111-111111111111".into(),
        operation_id: "22222222-2222-4222-8222-222222222222".into(),
        attempt: 1,
        base_text_revision: u64::MAX,
        base_catalog_digest: stable_digest(
            &catalog.files.iter().map(|f| &f.record).collect::<Vec<_>>(),
        )?,
        base_head: None,
        scope: BuildScope::Both,
        configuration,
        policy_fingerprint,
        rules_fingerprint: stable_digest(&"rules-v1")?,
        module_roots_fingerprint: stable_digest(&"roots-v1")?,
    };
    Ok((catalog, spec))
}

#[test]
fn build_root_receipt_confirm_replay_and_readonly_reopen() -> Result<(), Box<dyn std::error::Error>>
{
    let (catalog, spec) = build_fixture()?;
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("build");
    let store = GraphDbStore::create_build(&path, GraphLimits::default())?;
    store.write_facts(&catalog, &context())?;
    assert_eq!(store.inspect_build(&spec, &context())?, None);
    let confirmed = store.commit_build(&spec, &context())?;
    assert_eq!(
        store.inspect_published(&spec, &context())?,
        Some(confirmed.clone())
    );
    assert_eq!(confirmed.graph_build_id, spec.graph_build_id);
    assert_eq!(confirmed.facts_digest, stable_digest(&catalog)?);
    assert_eq!(store.commit_build(&spec, &context())?, confirmed);
    let mut changed = spec.clone();
    changed.attempt += 1;
    assert_eq!(
        store
            .commit_build(&changed, &context())
            .expect_err("changed attempt")
            .kind,
        GraphErrorKind::Conflict
    );
    changed = spec.clone();
    changed.base_catalog_digest = stable_digest(&"different base")?;
    assert_eq!(
        store
            .commit_build(&changed, &context())
            .expect_err("changed base")
            .kind,
        GraphErrorKind::Conflict
    );
    assert_eq!(
        store.inspect_build(&spec, &context())?,
        Some(confirmed.clone())
    );
    store.checkpoint()?;
    drop(store);
    let store = GraphDbStore::open(&path, true, GraphLimits::default())?;
    assert_eq!(store.inspect_build(&spec, &context())?, Some(confirmed));
    assert_eq!(
        store
            .commit_build(&spec, &context())
            .expect_err("read-only commit")
            .kind,
        GraphErrorKind::ReadOnly
    );
    Ok(())
}

#[test]
fn build_root_and_receipt_native_failure_roll_back_together()
-> Result<(), Box<dyn std::error::Error>> {
    let (catalog, spec) = build_fixture()?;
    let directory = tempfile::tempdir()?;
    let store = GraphDbStore::create_build(directory.path().join("build"), GraphLimits::default())?;
    store.write_facts(&catalog, &context())?;
    let mut inserted_pair = false;
    let error = store
        .with_write(|tx, _| {
            tx.execute(
                "CREATE (:GraphRoot {graph_build_id:$id})",
                params! {"id"=>spec.graph_build_id.as_str()},
            )?;
            for i in 0..2 {
                tx.execute(
                    "CREATE (:Receipt {receipt_id:$id})",
                    params! {"id"=>"duplicate"},
                )?;
                if i == 0 {
                    inserted_pair = true;
                }
            }
            Ok(())
        })
        .expect_err("real duplicate key failure after writing root and receipt");
    assert!(
        inserted_pair,
        "both rows must exist in the failed transaction before the injected duplicate"
    );
    assert_eq!(error.outcome, Some(GraphOutcome::RolledBack));
    assert_eq!(store.inspect_build(&spec, &context())?, None);
    store.commit_build(&spec, &context())?;
    store.with_write(|tx, _| {
        tx.execute("MATCH (r:Receipt) DELETE r", ())?;
        Ok(())
    })?;
    assert_eq!(
        store
            .inspect_build(&spec, &context())
            .expect_err("missing receipt is not proof")
            .kind,
        GraphErrorKind::IncompatibleFormat
    );
    Ok(())
}

#[test]
fn build_validation_and_precommit_cancel_leave_no_success_proof()
-> Result<(), Box<dyn std::error::Error>> {
    let (catalog, spec) = build_fixture()?;
    let mut future = serde_json::to_value(&spec)?;
    future["futureContractField"] = serde_json::json!(true);
    assert!(serde_json::from_value::<super::super::build::BuildSpec>(future).is_err());
    let directory = tempfile::tempdir()?;
    let store = GraphDbStore::create_build(directory.path().join("build"), GraphLimits::default())?;
    store.write_facts(&catalog, &context())?;
    let cancelled = context();
    cancelled.cancellation.cancel();
    assert_eq!(
        store
            .commit_build(&spec, &cancelled)
            .expect_err("cancel before effects")
            .kind,
        GraphErrorKind::Cancelled
    );
    let mut wrong = spec.clone();
    wrong.policy_fingerprint = stable_digest(&"wrong policy")?;
    assert_eq!(
        store
            .commit_build(&wrong, &context())
            .expect_err("bad fingerprint")
            .kind,
        GraphErrorKind::InvalidInput
    );
    wrong = spec.clone();
    wrong.scope = super::super::build::BuildScope::Code;
    assert_eq!(
        store
            .commit_build(&wrong, &context())
            .expect_err("document facts in code-only build")
            .kind,
        GraphErrorKind::InvalidInput
    );
    wrong = spec.clone();
    wrong.graph_build_id = "../escape".into();
    assert_eq!(
        store
            .commit_build(&wrong, &context())
            .expect_err("invalid UUID")
            .kind,
        GraphErrorKind::InvalidInput
    );
    assert_eq!(store.inspect_build(&spec, &context())?, None);
    let mut limited = context();
    limited.max_input_bytes = 0;
    assert_eq!(
        store
            .commit_build(&spec, &limited)
            .expect_err("metadata budget")
            .kind,
        GraphErrorKind::ResourceExhausted
    );
    Ok(())
}

#[test]
fn eight_tables_round_trip_reopen_and_resolve_without_source()
-> Result<(), Box<dyn std::error::Error>> {
    let catalog = fixture()?;
    assert!(
        catalog
            .resolutions
            .iter()
            .any(|r| r.status == ResolutionStatus::Resolved)
    );
    assert!(
        catalog
            .resolutions
            .iter()
            .any(|r| r.status == ResolutionStatus::Ambiguous)
    );
    assert!(
        catalog
            .resolutions
            .iter()
            .any(|r| r.status == ResolutionStatus::Unsupported)
    );
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("graph");
    let store = GraphDbStore::create(&path, GraphLimits::default())?;
    let counts = store.write_facts(&catalog, &context())?;
    for table in schema::FACT_TABLES {
        assert!(counts.nodes[*table] > 0, "{table}");
    }
    assert_eq!(store.read_facts(&context())?, catalog);
    assert_eq!(
        store
            .write_facts(&catalog, &context())
            .expect_err("write-once candidate")
            .kind,
        GraphErrorKind::Conflict
    );
    store.checkpoint()?;
    drop(store);
    let store = GraphDbStore::open(&path, true, GraphLimits::default())?;
    let reopened = store.read_facts(&context())?;
    assert_eq!(reopened, catalog);
    assert_eq!(store.counts()?, counts);
    assert_eq!(
        store
            .write_facts(&catalog, &context())
            .expect_err("read only")
            .kind,
        GraphErrorKind::ReadOnly
    );
    // Change only a target declaration in the persisted structure. No raw text
    // reread or AST extraction occurs; compare the same pure resolver inputs.
    let mut changed = reopened.files.clone();
    let target = changed
        .iter_mut()
        .find(|f| f.record.path == "b.md")
        .ok_or("target")?;
    let updated = "# Same\n# Different\n";
    let replacement = bind(
        extract_markdown(version("b.md", updated), updated, &context())?,
        updated,
        3,
    )?;
    *target = replacement.clone();
    let rerun = FactCatalog::resolve(
        changed.clone(),
        catalog.config_fingerprint.clone(),
        catalog.selection_fingerprint.clone(),
        &context(),
    )?;
    let mut fresh = fixture()?.files;
    *fresh
        .iter_mut()
        .find(|f| f.record.path == "b.md")
        .ok_or("fresh target")? = replacement;
    let full = FactCatalog::resolve(
        fresh,
        catalog.config_fingerprint.clone(),
        catalog.selection_fingerprint.clone(),
        &context(),
    )?;
    assert_eq!(rerun, full);
    assert!(
        rerun
            .resolutions
            .iter()
            .filter(|r| r.file_path == "a.md")
            .any(|r| r.status == ResolutionStatus::Resolved)
    );
    assert_ne!(rerun.catalogs, catalog.catalogs);
    Ok(())
}

#[test]
fn candidate_schema_protects_nonempty_interrupted_and_unknown_formats()
-> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let nonempty = directory.path().join("nonempty");
    fs::create_dir(&nonempty)?;
    fs::write(nonempty.join("owned"), b"keep")?;
    assert!(matches!(
        GraphDbStore::create(&nonempty, GraphLimits::default()),
        Err(GraphError {
            kind: GraphErrorKind::Conflict,
            ..
        })
    ));
    let path = directory.path().join("graph");
    let store = GraphDbStore::create(&path, GraphLimits::default())?;
    drop(store);
    assert!(
        GraphDbStore::open(&path, false, GraphLimits::default()).is_err(),
        "DDL-only has no ready metadata"
    );
    let store = GraphDbStore::open_native(&path, false, GraphLimits::default())?;
    store.write_facts(&fixture()?, &context())?;
    store.with_write(|tx, _| {
        tx.execute(
            "MATCH (m:KgFactMetadata) SET m.facts_format_version=999",
            (),
        )?;
        Ok(())
    })?;
    drop(store);
    assert!(matches!(
        GraphDbStore::open(&path, true, GraphLimits::default()),
        Err(GraphError {
            kind: GraphErrorKind::IncompatibleFormat,
            ..
        })
    ));
    Ok(())
}

#[test]
fn native_write_failure_rolls_back_and_edge_corruption_is_rejected()
-> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let store = GraphDbStore::create(directory.path().join("graph"), GraphLimits::default())?;
    let failure=store.with_write(|tx,_| {
        for _ in 0..2 {tx.execute("CREATE (:KgScope {id:$id,file_path:$path,ordinal:$ordinal,kind:$kind,owner_id:$owner})",params!{"id"=>"duplicate","path"=>"a.py","ordinal"=>0_u64,"kind"=>"module","owner"=>"missing"})?;}
        Ok(())
    }).expect_err("duplicate aborts entire write");
    assert_eq!(failure.outcome, Some(GraphOutcome::RolledBack));
    assert_eq!(store.counts()?.nodes["KgScope"], 0);
    store.write_facts(&fixture()?, &context())?;
    store.with_write(|tx, _| {
        tx.execute("MATCH ()-[e:KgScopeParent]->() DELETE e", ())?;
        Ok(())
    })?;
    assert!(
        store.read_facts(&context()).is_err(),
        "missing structural edge cannot become complete facts"
    );
    Ok(())
}

#[test]
fn primary_key_edge_writes_respect_small_row_budgets() -> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let store = GraphDbStore::create(
        directory.path().join("graph"),
        GraphLimits {
            max_intermediate_rows: 16,
            ..GraphLimits::default()
        },
    )?;
    // A WHERE filter after a two-table product would exceed 16 (7 nodes x
    // 4 references). PK properties seek each endpoint before combining them.
    let catalog = fixture()?;
    store.write_facts(&catalog, &context())?;
    assert_eq!(store.read_facts(&context())?, catalog);
    Ok(())
}

#[test]
fn cancellation_and_aggregate_budget_reject_before_mutation()
-> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let store = GraphDbStore::create(directory.path().join("graph"), GraphLimits::default())?;
    let cancelled = context();
    cancelled.cancellation.cancel();
    assert_eq!(
        store
            .write_facts(&fixture()?, &cancelled)
            .expect_err("cancelled")
            .kind,
        GraphErrorKind::Cancelled
    );
    assert!(store.counts()?.nodes.values().all(|n| *n == 0));
    drop(store);
    let limited = GraphDbStore::create(
        directory.path().join("limited"),
        GraphLimits {
            max_rows: 3,
            ..GraphLimits::default()
        },
    )?;
    assert_eq!(
        limited
            .write_facts(&fixture()?, &context())
            .expect_err("aggregate budget")
            .kind,
        GraphErrorKind::ResourceExhausted
    );
    assert!(limited.counts()?.nodes.values().all(|n| *n == 0));
    Ok(())
}

fn disk_bytes(path: &Path) -> std::io::Result<u64> {
    fs::read_dir(path)?.try_fold(0, |n, e| {
        let p = e?.path();
        Ok(n + if p.is_dir() {
            disk_bytes(&p)?
        } else {
            p.metadata()?.len()
        })
    })
}
#[test]
fn structure_cost_probe() -> Result<(), Box<dyn std::error::Error>> {
    // Default exercises the real probe with a small deterministic fixture.
    // The documented L1 run supplies an immutable, digest-recorded host capture.
    let supplied = std::env::var_os("ZG_G2_FACT_CATALOG");
    let catalog: FactCatalog = if let Some(path) = &supplied {
        serde_json::from_slice(&fs::read(path)?)?
    } else {
        fixture()?
    };
    catalog.validate(&context())?;
    let rows = FactRows::encode(&catalog)?;
    let root = tempfile::tempdir()?;
    let limits = GraphLimits {
        max_rows: 4_000_000,
        ..GraphLimits::default()
    };
    let mut runs = Vec::new();
    for full in [false, true] {
        let path = root.path().join(if full { "full" } else { "base" });
        let store = GraphDbStore::create(&path, limits.clone())?;
        let before = disk_bytes(&path)?;
        let started = Instant::now();
        store.with_write(|tx, options| rows.write(tx, options, &context(), full))?;
        let write_us = started.elapsed().as_micros();
        let counts = store.counts()?;
        let started = Instant::now();
        store.checkpoint()?;
        let checkpoint_us = started.elapsed().as_micros();
        drop(store);
        runs.push(serde_json::json!({"structure":full,"write_us":write_us,"checkpoint_us":checkpoint_us,"empty_bytes":before,"bytes":disk_bytes(&path)?,"counts":counts}));
    }
    let report = serde_json::json!({"corpus":if supplied.is_some(){"L1 host capture"}else{"unit fixture"},"files":catalog.files.len(),"catalog_digest":stable_digest(&catalog)?,"runs":runs,
        "note":"fresh directories; single sequential sample; baseline omits three structure tables and five auxiliary relations' rows, retains identical empty DDL; no performance Go claim"});
    if let Some(path) = std::env::var_os("ZG_G2_VOLUME_REPORT") {
        fs::write(path, serde_json::to_vec_pretty(&report)?)?;
    }
    eprintln!("G2_1_VOLUME {}", serde_json::to_string(&report)?);
    let base = &report["runs"][0]["counts"];
    let full = &report["runs"][1]["counts"];
    for t in schema::STRUCTURE_TABLES {
        assert_eq!(base["nodes"][*t], 0);
    }
    for t in schema::FACT_TABLES
        .iter()
        .filter(|t| !schema::STRUCTURE_TABLES.contains(t))
    {
        assert_eq!(base["nodes"][*t], full["nodes"][*t]);
    }
    for t in schema::STRUCTURE_RELATIONS {
        assert_eq!(base["relations"][*t], 0);
    }
    Ok(())
}

#[path = "import_tests.rs"]
mod import_tests;

#[path = "control_tests.rs"]
mod control_tests;

#[test]
fn business_pages_and_streaming_proof_cover_parallel_edges_and_native_ref_ids()
-> Result<(), Box<dyn std::error::Error>> {
    use crate::query::{
        BudgetLimits, Direction, GraphReader, ReferencePartition, RelationKind as QKind,
        RequestContext,
    };
    let (catalog, spec) = build_fixture()?;
    let mut files = catalog.files;
    let source = files
        .iter_mut()
        .find(|f| f.record.path == "a.py")
        .ok_or("source")?;
    let facts = source.facts.as_mut().ok_or("facts")?;
    let owner = facts.nodes[1].id.clone();
    for _ in 0..300 {
        facts.add_reference(
            1,
            RelationKind::Call,
            Span { start: 0, end: 6 },
            Lookup::Name {
                parts: vec!["helper".into()],
            },
        )?;
        source
            .reference_entities
            .push(source.node_entities[1].clone());
    }
    let catalog = FactCatalog::resolve(
        files,
        catalog.config_fingerprint,
        catalog.selection_fingerprint,
        &context(),
    )?;
    let dir = tempfile::tempdir()?;
    let store = GraphDbStore::create_build(dir.path().join("graph"), GraphLimits::default())?;
    store.write_facts(&catalog, &context())?;
    store.commit_build(&spec, &context())?;
    assert!(store.inspect_published(&spec, &context())?.is_some());
    let ctx = RequestContext::new(
        BudgetLimits::default(),
        tokio_util::sync::CancellationToken::new(),
        Instant::now() + std::time::Duration::from_secs(30),
    )?;
    let first = store.neighbors(
        std::slice::from_ref(&owner),
        QKind::Calls,
        Direction::Outgoing,
        0,
        &ctx,
    )?;
    let second = store.neighbors(
        std::slice::from_ref(&owner),
        QKind::Calls,
        Direction::Outgoing,
        256,
        &ctx,
    )?;
    assert_eq!(first.len(), 256);
    assert_eq!(second.len(), 45);
    let ids = first
        .iter()
        .chain(&second)
        .map(|r| r.id.clone())
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(ids.len(), 301);
    assert_eq!(
        store
            .references(ReferencePartition::Calls, Some(&owner), None, None, 0, &ctx)?
            .len(),
        256
    );
    assert_eq!(
        store
            .references(
                ReferencePartition::Calls,
                Some(&owner),
                None,
                None,
                256,
                &ctx
            )?
            .len(),
        45
    );
    let id = &first[0].id;
    store.with_write(|tx, options| {
        tx.execute_with(
            "MATCH ()-[e:KgCalls]->() WHERE e.ref_id=$id SET e.ref_id=$bad",
            params! {"id"=>id,"bad"=>"changed-native-ref-id"},
            options,
        )?;
        Ok(())
    })?;
    assert!(
        store.inspect_published(&spec, &context()).is_err(),
        "indexed property is part of the integrity check"
    );
    Ok(())
}

#[test]
fn ordered_integrity_rejects_changes_to_every_fact_table_and_private_edges()
-> Result<(), Box<dyn std::error::Error>> {
    let (catalog, spec) = build_fixture()?;
    let mut mutations = vec![
        "MATCH(n:KgFile) SET n.size_bytes=0".to_owned(),
        "MATCH(n:KgEntity) SET n.name='tampered'".to_owned(),
        "MATCH(n:KgUnresolvedRef) SET n.reason='tampered'".to_owned(),
        "MATCH(n:KgAnchor) SET n.slug='tampered'".to_owned(),
        "MATCH(n:KgCatalogRevision) SET n.members_digest='tampered'".to_owned(),
        "MATCH(n:KgScope) SET n.kind='tampered'".to_owned(),
        "MATCH(n:KgBinding) SET n.name='tampered'".to_owned(),
        "MATCH(n:KgDiagnostic) SET n.code='tampered'".to_owned(),
        "MATCH(n:KgFactMetadata) SET n.rows_digest='tampered'".to_owned(),
        "MATCH(n:GraphRoot) SET n.facts_digest='tampered'".to_owned(),
        "MATCH(n:Receipt) SET n.root_digest='tampered'".to_owned(),
        "CREATE(:KgScope {id:'extra-uncommitted-row'})".to_owned(),
        "MATCH()-[e:KgScopeParent]->() DELETE e".to_owned(),
    ];
    for (name, source, target) in [
        ("KgScopeParent", "KgScope", "KgScope"),
        ("KgScopeOf", "KgScope", "KgEntity"),
        ("KgInScope", "KgBinding", "KgScope"),
        ("KgBindNode", "KgBinding", "KgEntity"),
        ("KgDiagOf", "KgDiagnostic", "KgFile"),
    ] {
        mutations.push(format!(
            "MATCH(a:{source})-[:{name}]->(b:{target}) CREATE(a)-[:{name}]->(b)"
        ));
    }
    for mutation in mutations {
        let dir = tempfile::tempdir()?;
        let store = GraphDbStore::create_build(dir.path().join("graph"), GraphLimits::default())?;
        store.write_facts(&catalog, &context())?;
        store.commit_build(&spec, &context())?;
        assert!(store.inspect_published(&spec, &context())?.is_some());
        store.with_write(|tx, options| {
            tx.execute_with(&mutation, (), options)?;
            Ok(())
        })?;
        assert!(
            !matches!(store.inspect_published(&spec, &context()), Ok(Some(_))),
            "must reject {mutation}"
        );
    }
    Ok(())
}

#[test]
fn typed_integrity_bytes_match_the_existing_v4_canonical_encoding()
-> Result<(), Box<dyn std::error::Error>> {
    let (catalog, _) = build_fixture()?;
    let rows = super::codec::FactRows::encode(&catalog)?;
    let mut previous = stable_digest(&"kg-rows-v4")?;
    let mut scratch = Vec::new();
    for mutation in rows.mutations() {
        let expected = crate::processing::canonical_json(&(&previous, &mutation))?;
        let next = mutation.update_digest(&previous, &mut scratch)?;
        assert_eq!(scratch, expected.as_bytes());
        assert_eq!(next, stable_digest(&(&previous, &mutation))?);
        previous = next;
    }
    assert_eq!(previous, rows.digest()?);
    let mut node = rows
        .mutations()
        .into_iter()
        .find_map(|mutation| match mutation {
            super::codec::Mutation::Node(node) => Some(node.clone()),
            _ => None,
        })
        .expect("fixture node");
    node.name.push_str("\0\u{1}\n\r\t\\\"中😀");
    let mutation = super::codec::Mutation::Node(&node);
    let expected = crate::processing::canonical_json(&(&previous, &mutation))?;
    assert_eq!(
        mutation.update_digest(&previous, &mut scratch)?,
        stable_digest(&(&previous, &mutation))?
    );
    assert_eq!(scratch, expected.as_bytes());
    Ok(())
}
