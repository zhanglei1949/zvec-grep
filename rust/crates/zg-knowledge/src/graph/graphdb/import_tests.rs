use super::super::ImportBudget;
use super::*;

#[test]
fn finalized_import_retries_and_root_commit_recheck_persisted_semantic_facts()
-> Result<(), Box<dyn std::error::Error>> {
    let (catalog, spec) = super::super::semantic_tests::semantic_fixture()?;
    let dir = tempfile::tempdir()?;
    let store =
        GraphDbStore::create_batched_build(dir.path().join("graph"), GraphLimits::default())?;
    let budget = ImportBudget::default();
    let ready = store.write_facts_batched(&catalog, &budget, &context())?;
    assert!(ready.finalized);
    assert_eq!(
        store.write_facts_batched(&catalog, &budget, &context())?,
        ready
    );
    assert_eq!(store.read_build_facts(&spec, &context())?, None);
    let changed_budget = ImportBudget {
        max_batch_rows: budget.max_batch_rows + 1,
        ..budget.clone()
    };
    assert_eq!(
        store
            .write_facts_batched(&catalog, &changed_budget, &context())
            .expect_err("finalized imports keep their saved budget")
            .kind,
        GraphErrorKind::Conflict
    );
    let mut semantic = catalog.semantic.clone();
    semantic.entities[0].name.push_str(" changed");
    let changed = catalog.clone().with_semantic(semantic, &context())?;
    assert_eq!(
        store
            .write_facts_batched(&changed, &budget, &context())
            .expect_err("finalized facts cannot be rebound to changed input")
            .kind,
        GraphErrorKind::Conflict
    );
    store.with_write(|tx, options| {
        tx.execute_with(
            "MATCH(n:KgSemanticEntity) SET n.name='corrupted'",
            (),
            options,
        )?;
        Ok(())
    })?;
    assert!(
        store
            .write_facts_batched(&catalog, &budget, &context())
            .is_err()
    );
    assert!(store.commit_build(&spec, &context()).is_err());
    store.ensure_no_build_commit()?;
    Ok(())
}

#[test]
fn build_fact_read_verifies_fresh_rows_root_and_receipt() -> Result<(), Box<dyn std::error::Error>>
{
    for query in [
        "MATCH(n:KgSemanticEntity) SET n.name='corrupted'",
        "MATCH(r:GraphRoot) SET r.attempt=2",
        "MATCH(r:Receipt) DELETE r",
    ] {
        let (catalog, spec) = super::super::semantic_tests::semantic_fixture()?;
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("graph");
        let store = GraphDbStore::create_batched_build(&path, GraphLimits::default())?;
        store.write_facts_batched(&catalog, &ImportBudget::default(), &context())?;
        store.commit_build(&spec, &context())?;
        store.checkpoint()?;
        drop(store);
        let store = GraphDbStore::open(&path, false, GraphLimits::default())?;
        assert_eq!(store.read_build_facts(&spec, &context())?, Some(catalog));
        let mut changed = spec.clone();
        changed.attempt += 1;
        assert_eq!(
            store
                .read_build_facts(&changed, &context())
                .expect_err("changed proof")
                .kind,
            GraphErrorKind::Conflict
        );
        let cancelled = context();
        cancelled.cancellation.cancel();
        assert_eq!(
            store
                .read_build_facts(&spec, &cancelled)
                .expect_err("cancelled read")
                .kind,
            GraphErrorKind::Cancelled
        );
        store.with_write(|tx, options| {
            tx.execute_with(query, (), options)?;
            Ok(())
        })?;
        assert!(
            store.read_build_facts(&spec, &context()).is_err(),
            "{query}"
        );
    }
    Ok(())
}

#[test]
fn batched_admission_refusal_preserves_native_prefix_and_can_resume()
-> Result<(), Box<dyn std::error::Error>> {
    use std::cell::Cell;
    for refuse_finalization in [false, true] {
        let (catalog, _) = build_fixture()?;
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("graph");
        let store = GraphDbStore::create_batched_build(&path, GraphLimits::default())?;
        let budget = ImportBudget {
            max_batch_rows: 3,
            max_batch_bytes: 4096,
        };
        let stop = Cell::new(false);
        let error = store
            .write_facts_batched_admitted(
                &catalog,
                &budget,
                &context(),
                || {
                    if stop.get() {
                        Err(GraphError::from(crate::Error::ResourceExhausted))
                    } else {
                        Ok(())
                    }
                },
                |p| stop.set(!refuse_finalization || p.completed_batches == p.total_batches),
            )
            .expect_err("budget stopped before next transaction");
        assert_eq!(error.kind, GraphErrorKind::ResourceExhausted);
        let progress = store.inspect_import(&context())?;
        assert!(!progress.finalized);
        assert!(progress.committed_rows > 0);
        if refuse_finalization {
            assert_eq!(progress.completed_batches, progress.total_batches);
        } else {
            assert_eq!(progress.completed_batches, 1);
        }
        store.ensure_no_build_commit()?;
        drop(store);
        let store = GraphDbStore::open_import(&path, false, GraphLimits::default(), &context())?;
        assert_eq!(store.inspect_import(&context())?, progress);
        store.write_facts_batched(&catalog, &budget, &context())?;
        assert_eq!(store.read_facts(&context())?, catalog);
    }
    Ok(())
}

#[test]
fn borrowed_mutations_and_root_keep_the_v8_digest_contract()
-> Result<(), Box<dyn std::error::Error>> {
    let (catalog, spec) = build_fixture()?;
    let rows = FactRows::encode(&catalog)?;
    let mut original = stable_digest(&"kg-rows-v8")?;
    assert_eq!(
        super::super::codec::catalog_digest(&catalog)?,
        stable_digest(&catalog)?
    );
    for row in rows.mutations() {
        original = stable_digest(&(original, row))?;
    }
    assert_eq!(rows.digest()?, original);
    let dir = tempfile::tempdir()?;
    let store =
        GraphDbStore::create_batched_build(dir.path().join("graph"), GraphLimits::default())?;
    store.write_facts_batched(&catalog, &ImportBudget::default(), &context())?;
    let proof = store.commit_build(&spec, &context())?;
    assert_eq!(proof.facts_digest, stable_digest(&catalog)?);
    store.with_read(|tx, options| {
        let roots = crate::graph::schema::RootRow::read(tx, options)?;
        assert_eq!(roots[0].rows_digest, original);
        assert_eq!(roots[0].facts_digest, proof.facts_digest);
        Ok(())
    })?;
    assert_eq!(store.inspect_build(&spec, &context())?, Some(proof));
    Ok(())
}

#[test]
fn indexed_decode_rejects_orphan_rows_and_duplicate_ordinals()
-> Result<(), Box<dyn std::error::Error>> {
    for query in [
        "MATCH(n:KgEntity) SET n.file_path='missing-source.rs'",
        "MATCH(n:KgEntity) SET n.ordinal=0",
    ] {
        let dir = tempfile::tempdir()?;
        let store = GraphDbStore::create(dir.path().join("graph"), GraphLimits::default())?;
        store.write_facts(&fixture()?, &context())?;
        store.with_write(|tx, options| {
            tx.execute_with(query, (), options)?;
            Ok(())
        })?;
        assert!(store.read_facts(&context()).is_err());
    }
    Ok(())
}

#[test]
fn finalized_import_rejects_changed_input_as_conflict() -> Result<(), Box<dyn std::error::Error>> {
    let catalog = fixture()?;
    let dir = tempfile::tempdir()?;
    let store =
        GraphDbStore::create_batched_build(dir.path().join("graph"), GraphLimits::default())?;
    let budget = ImportBudget::default();
    let ready = store.write_facts_batched(&catalog, &budget, &context())?;
    let changed = FactCatalog::resolve(
        catalog.files.clone(),
        catalog.config_fingerprint.clone(),
        stable_digest(&"different-selection")?,
        &context(),
    )?;
    assert_eq!(
        store
            .write_facts_batched(&changed, &budget, &context())
            .expect_err("different input plan")
            .kind,
        GraphErrorKind::Conflict
    );
    assert_eq!(store.inspect_import(&context())?, ready);
    assert_eq!(
        store.write_facts_batched(&catalog, &budget, &context())?,
        ready
    );
    assert_eq!(store.read_facts(&context())?, catalog);
    Ok(())
}

#[test]
fn legacy_default_budget_resumes_after_default_changes() -> Result<(), Box<dyn std::error::Error>> {
    let (catalog, spec) = build_fixture()?;
    let mut files = catalog.files;
    let source = files
        .iter_mut()
        .find(|f| f.record.path == "a.py")
        .ok_or("source")?;
    let facts = source.facts.as_mut().ok_or("facts")?;
    for _ in 0..3000 {
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
    for legacy in [
        ImportBudget {
            max_batch_rows: 256,
            max_batch_bytes: 256 * 1024,
        },
        ImportBudget {
            max_batch_rows: 2048,
            max_batch_bytes: 2 * 1024 * 1024,
        },
    ] {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("graph");
        let store = GraphDbStore::create_batched_build(&path, GraphLimits::default())?;
        let ctx = context();
        let cancel = ctx.cancellation.clone();
        let error = store
            .write_facts_batched_observed(&catalog, &legacy, &ctx, |_| cancel.cancel())
            .expect_err("stop after the first legacy batch");
        assert_eq!(error.kind, GraphErrorKind::Cancelled);
        let prefix = store.inspect_import(&context())?;
        assert_eq!(prefix.completed_batches, 1);
        assert!(prefix.completed_batches < prefix.total_batches);
        assert_eq!(prefix.budget, legacy);
        drop(store);

        let store = GraphDbStore::open_import(&path, false, GraphLimits::default(), &context())?;
        assert_eq!(store.inspect_import(&context())?, prefix);
        assert_ne!(ImportBudget::default(), legacy);
        assert_eq!(
            store
                .write_facts_batched(&catalog, &ImportBudget::default(), &context())
                .expect_err("a new default must not re-partition an existing candidate")
                .kind,
            GraphErrorKind::Conflict,
        );
        assert_eq!(store.inspect_import(&context())?, prefix);
        assert!(
            store
                .write_facts_batched(&catalog, &prefix.budget, &context())?
                .finalized
        );
        let proof = store.commit_build(&spec, &context())?;
        store.checkpoint()?;
        drop(store);
        let store = GraphDbStore::open(&path, true, GraphLimits::default())?;
        assert_eq!(store.read_facts(&context())?, catalog);
        assert_eq!(store.inspect_build(&spec, &context())?, Some(proof));
    }
    Ok(())
}

#[test]
fn batched_import_cancel_reopen_resume_and_reject_changed_plan()
-> Result<(), Box<dyn std::error::Error>> {
    let (catalog, spec) = build_fixture()?;
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("graph");
    let budget = ImportBudget {
        max_batch_rows: 3,
        max_batch_bytes: 4096,
    };
    let store = GraphDbStore::create_batched_build(&path, GraphLimits::default())?;
    let ctx = context();
    let cancel = ctx.cancellation.clone();
    let error = store
        .write_facts_batched_observed(&catalog, &budget, &ctx, |p| {
            assert_eq!(p.completed_batches, 1);
            cancel.cancel();
        })
        .expect_err("cancel after first committed batch");
    assert_eq!(error.kind, GraphErrorKind::Cancelled);
    let progress = store.inspect_import(&context())?;
    assert_eq!(progress.committed_rows, 3);
    assert!(!progress.finalized);
    store.ensure_no_build_commit()?;
    store.checkpoint()?;
    drop(store);
    assert!(GraphDbStore::open(&path, true, GraphLimits::default()).is_err());
    let store = GraphDbStore::open_import(&path, false, GraphLimits::default(), &context())?;
    let changed = ImportBudget {
        max_batch_rows: 4,
        ..budget.clone()
    };
    assert_eq!(
        store
            .write_facts_batched(&catalog, &changed, &context())
            .expect_err("fixed plan")
            .kind,
        GraphErrorKind::Conflict
    );
    assert_eq!(store.inspect_import(&context())?, progress);
    let changed_catalog = FactCatalog::resolve(
        catalog.files.clone(),
        catalog.config_fingerprint.clone(),
        stable_digest(&"changed-selection")?,
        &context(),
    )?;
    assert_eq!(
        store
            .write_facts_batched(&changed_catalog, &budget, &context())
            .expect_err("cannot rebind input")
            .kind,
        GraphErrorKind::Conflict
    );
    let ready = store.write_facts_batched(&catalog, &budget, &context())?;
    assert!(ready.finalized);
    assert!(ready.total_batches > 2);
    assert_eq!(ready.committed_rows, ready.total_rows);
    assert_eq!(store.read_facts(&context())?, catalog);
    let proof = store.commit_build(&spec, &context())?;
    store.checkpoint()?;
    drop(store);
    let store = GraphDbStore::open_import(&path, true, GraphLimits::default(), &context())?;
    assert_eq!(store.inspect_build(&spec, &context())?, Some(proof));
    assert_eq!(
        store
            .write_facts_batched(&catalog, &budget, &context())
            .expect_err("read only")
            .kind,
        GraphErrorKind::ReadOnly
    );
    Ok(())
}

#[test]
fn partial_import_progress_cannot_hide_missing_native_rows()
-> Result<(), Box<dyn std::error::Error>> {
    let catalog = fixture()?;
    let dir = tempfile::tempdir()?;
    let store =
        GraphDbStore::create_batched_build(dir.path().join("graph"), GraphLimits::default())?;
    let ctx = context();
    let cancel = ctx.cancellation.clone();
    store
        .write_facts_batched_observed(
            &catalog,
            &ImportBudget {
                max_batch_rows: 2,
                max_batch_bytes: 4096,
            },
            &ctx,
            |_| cancel.cancel(),
        )
        .expect_err("stop after first committed batch");
    store.with_write(|tx, options| {
        tx.execute_with("MATCH (f:KgFile) DELETE f", (), options)?;
        Ok(())
    })?;
    assert_eq!(
        store
            .inspect_import(&context())
            .expect_err("progress is not evidence for missing rows")
            .kind,
        GraphErrorKind::IncompatibleFormat
    );
    Ok(())
}

#[test]
fn oversized_row_and_exhausted_budget_leave_no_partial_facts()
-> Result<(), Box<dyn std::error::Error>> {
    let catalog = fixture()?;
    let dir = tempfile::tempdir()?;
    let empty = dir.path().join("empty");
    std::fs::create_dir(&empty)?;
    assert_eq!(
        GraphDbStore::open_import(&empty, false, GraphLimits::default(), &context())
            .err()
            .expect("do not initialize an empty recovery path")
            .kind,
        GraphErrorKind::IncompatibleFormat
    );
    assert!(std::fs::read_dir(&empty)?.next().is_none());
    let store =
        GraphDbStore::create_batched_build(dir.path().join("graph"), GraphLimits::default())?;
    assert_eq!(
        store
            .write_facts_batched(
                &catalog,
                &ImportBudget {
                    max_batch_rows: 1,
                    max_batch_bytes: 8
                },
                &context()
            )
            .expect_err("one row cannot fit")
            .kind,
        GraphErrorKind::ResourceExhausted
    );
    let mut ctx = context();
    ctx.max_input_bytes = 4;
    assert_eq!(
        store
            .write_facts_batched(&catalog, &ImportBudget::default(), &ctx)
            .expect_err("aggregate encoded budget")
            .kind,
        GraphErrorKind::ResourceExhausted
    );
    assert_eq!(store.counts()?.nodes.values().sum::<u64>(), 0);
    let ready = store.write_facts_batched(
        &catalog,
        &ImportBudget {
            max_batch_rows: 100_000,
            max_batch_bytes: 2048,
        },
        &context(),
    )?;
    assert!(ready.finalized);
    assert!(ready.total_batches > 1);
    Ok(())
}

#[test]
fn typed_batches_match_row_writes_including_parallel_edges()
-> Result<(), Box<dyn std::error::Error>> {
    let (catalog, _) = build_fixture()?;
    let mut files = catalog.files;
    let source = files
        .iter_mut()
        .find(|file| file.record.path == "a.py")
        .ok_or("source")?;
    let facts = source.facts.as_mut().ok_or("facts")?;
    for _ in 0..20 {
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
    let expected = FactRows::encode(&catalog)?;
    let dir = tempfile::tempdir()?;
    let row_store = GraphDbStore::create(dir.path().join("rows"), GraphLimits::default())?;
    let batch_store =
        GraphDbStore::create_batched_build(dir.path().join("batches"), GraphLimits::default())?;
    let expected_counts = row_store.write_facts(&catalog, &context())?;
    let progress = batch_store.write_facts_batched(
        &catalog,
        &ImportBudget {
            max_batch_rows: 3,
            max_batch_bytes: 4096,
        },
        &context(),
    )?;
    assert!(progress.finalized);
    assert_eq!(batch_store.counts()?, expected_counts);
    assert_eq!(batch_store.read_facts(&context())?, catalog);
    for store in [&row_store, &batch_store] {
        store.with_read(|tx, options| {
            assert_eq!(FactRows::read(tx, options)?, expected);
            Ok(())
        })?;
    }
    assert!(expected_counts.relations["KgCalls"] >= 21);
    Ok(())
}

#[test]
fn typed_batch_failure_rolls_back_all_tables_in_the_logical_batch()
-> Result<(), Box<dyn std::error::Error>> {
    use super::super::codec::Mutation;
    let catalog = fixture()?;
    let rows = FactRows::encode(&catalog)?;
    let mutations = rows.mutations();
    let duplicate = mutations
        .iter()
        .find_map(|mutation| match mutation {
            Mutation::Node(row) => Some((*row).clone()),
            _ => None,
        })
        .expect("fixture node");
    let dir = tempfile::tempdir()?;
    let store = GraphDbStore::create(dir.path().join("graph"), GraphLimits::default())?;
    let error = store
        .with_write(|tx, options| {
            duplicate.insert(tx, options)?;
            FactRows::import_batch(&mutations, tx, options, &context())
        })
        .expect_err("duplicate key aborts the typed batch");
    assert_eq!(error.outcome, Some(GraphOutcome::RolledBack));
    let counts = store.counts()?;
    assert!(counts.nodes.values().all(|count| *count == 0));
    assert!(counts.relations.values().all(|count| *count == 0));
    Ok(())
}

#[test]
fn file_mapping_larger_than_copy_record_limit_round_trips_in_logical_batch()
-> Result<(), Box<dyn std::error::Error>> {
    use super::super::codec::Mutation;
    let rows = FactRows::encode(&fixture()?)?;
    let mut file = rows
        .mutations()
        .into_iter()
        .find_map(|m| match m {
            Mutation::File(row) => Some(row.clone()),
            _ => None,
        })
        .expect("fixture file");
    // Real ripgrep maps reach 183,780 bytes, within the 256 KiB logical batch
    // budget but above the native COPY reader's 128 KiB record limit.
    file.text_map = serde_json::to_string(&vec!["map"; 30_000])?;
    assert!(file.text_map.len() > 128 * 1024);
    assert!(
        serde_json::to_vec(&Mutation::File(&file))?.len()
            <= ImportBudget::default().max_batch_bytes
    );
    let dir = tempfile::tempdir()?;
    let store = GraphDbStore::create(dir.path().join("graph"), GraphLimits::default())?;
    store.with_write(|tx, options| {
        FactRows::import_batch(&[Mutation::File(&file)], tx, options, &context())
    })?;
    store.with_read(|tx, options| {
        let actual = FactRows::read(tx, options)?;
        let stored = actual
            .mutations()
            .into_iter()
            .find_map(|m| match m {
                Mutation::File(row) => Some(row),
                _ => None,
            })
            .expect("stored file");
        assert_eq!(stored, &file);
        Ok(())
    })?;
    Ok(())
}

#[test]
fn batched_import_process_exit_keeps_only_committed_prefix()
-> Result<(), Box<dyn std::error::Error>> {
    const CHILD: &str = "ZG_BATCH_IMPORT_EXIT_CHILD";
    let budget = ImportBudget {
        max_batch_rows: 3,
        max_batch_bytes: 4096,
    };
    let catalog = fixture()?;
    if let Some(path) = std::env::var_os(CHILD) {
        let store = GraphDbStore::create_batched_build(path, GraphLimits::default())?;
        store.write_facts_batched_observed(&catalog, &budget, &context(), |_| {
            std::process::exit(23)
        })?;
        panic!("child must exit at first confirmed batch");
    }
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("graph");
    let name = "graph::graphdb::tests::import_tests::batched_import_process_exit_keeps_only_committed_prefix";
    let status = std::process::Command::new(std::env::current_exe()?)
        .args(["--exact", name, "--nocapture"])
        .env(CHILD, &path)
        .status()?;
    assert_eq!(status.code(), Some(23));
    let store = GraphDbStore::open_import(&path, false, GraphLimits::default(), &context())?;
    assert_eq!(store.inspect_import(&context())?.committed_rows, 3);
    store.ensure_no_build_commit()?;
    assert!(
        store
            .write_facts_batched(&catalog, &budget, &context())?
            .finalized
    );
    assert_eq!(store.read_facts(&context())?, catalog);
    Ok(())
}
