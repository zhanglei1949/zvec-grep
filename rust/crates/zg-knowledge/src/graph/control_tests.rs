use super::*;
use crate::graph::{QueryControl, QueryStopReason};
use ::graphdb::{Batch, BatchSource, Column, ImportOptions, LogicalType, Schema};
use std::sync::{Arc, mpsc};

#[test]
fn first_stop_reason_preserves_unknown_and_committed_outcomes() -> GraphResult<()> {
    let control = QueryControl::new()?;
    control.request(QueryStopReason::Deadline);
    control.request(QueryStopReason::Cancelled);
    assert_eq!(control.reason(), Some(QueryStopReason::Deadline));
    let mut error = GraphError::local(GraphErrorKind::Cancelled);
    error.native_kind = Some("Cancelled".into());
    error.outcome = Some(GraphOutcome::Committed);
    let classified = control.classify(error);
    assert_eq!(classified.kind, GraphErrorKind::Deadline);
    assert_eq!(classified.outcome, Some(GraphOutcome::Committed));
    assert_eq!(classified.native_kind.as_deref(), Some("Cancelled"));
    let mut unknown = GraphError::local(GraphErrorKind::RecoveryRequired);
    unknown.outcome = Some(GraphOutcome::Unknown);
    let classified = control.classify(unknown);
    assert_eq!(classified.kind, GraphErrorKind::RecoveryRequired);
    assert_eq!(classified.outcome, Some(GraphOutcome::Unknown));
    Ok(())
}

/// Deterministic cancellation while a real native import is active, after its
/// first batch. This tests the query bridge, not arbitrary CPU preemption.
struct GatedSource {
    schema: Arc<Schema>,
    next: usize,
    entered: mpsc::Sender<()>,
    released: mpsc::Receiver<()>,
}
impl BatchSource for GatedSource {
    fn schema(&self) -> &Schema {
        &self.schema
    }
    fn next_batch(&mut self) -> ::graphdb::Result<Option<Batch>> {
        self.next += 1;
        if self.next == 1 {
            return Batch::try_new(self.schema.clone(), 1, vec![vec![1_i64].into()]).map(Some);
        }
        assert_eq!(self.next, 2);
        self.entered.send(()).expect("native call entered");
        self.released
            .recv_timeout(Duration::from_secs(10))
            .expect("stop requested");
        Batch::try_new(
            self.schema.clone(),
            0,
            vec![Column::from(Vec::<i64>::new())],
        )
        .map(Some)
    }
}

#[test]
fn active_native_import_cancel_and_deadline_roll_back() -> Result<(), Box<dyn std::error::Error>> {
    for reason in [QueryStopReason::Cancelled, QueryStopReason::Deadline] {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("graph");
        let mut store = GraphDbStore::create_build(&path, GraphLimits::default())?;
        let mut connection = store.database.connect()?;
        connection.execute(
            "CREATE NODE TABLE CancellationProbe(id INT64 PRIMARY KEY)",
            (),
        )?;
        connection.close()?;
        let control = QueryControl::new()?;
        store.limits.control = Some(control.clone());
        let (entered, wait) = mpsc::channel();
        let (release, released) = mpsc::channel();
        let signaller = std::thread::spawn(move || {
            wait.recv_timeout(Duration::from_secs(10))
                .expect("active native producer");
            control.request(reason);
            release.send(()).expect("release callback");
        });
        let source = GatedSource {
            schema: Arc::new(Schema::try_new(vec![("id".into(), LogicalType::Int64)])?),
            next: 0,
            entered,
            released,
        };
        let error = store
            .with_write(|tx, options| {
                tx.import_batches(
                    "CancellationProbe",
                    source,
                    ImportOptions {
                        query: options.clone(),
                        ..ImportOptions::default()
                    },
                )?;
                Ok(())
            })
            .expect_err("native cancellation");
        signaller.join().expect("signaller");
        assert_eq!(
            error.kind,
            match reason {
                QueryStopReason::Cancelled => GraphErrorKind::Cancelled,
                QueryStopReason::Deadline => GraphErrorKind::Deadline,
            }
        );
        assert_eq!(error.native_kind.as_deref(), Some("Cancelled"));
        assert_eq!(error.outcome, Some(GraphOutcome::RolledBack));
        drop(store);
        let reopened = GraphDbStore::open_native(&path, false, GraphLimits::default())?;
        let count: i64 = reopened.with_read(|tx, options| {
            let rows = tx.query_with("MATCH (n:CancellationProbe) RETURN count(*)", (), options)?;
            Ok(rows.rows().next()?.expect("count row").try_get(0)?)
        })?;
        assert_eq!(count, 0);
        assert_eq!(reopened.counts()?.nodes.values().sum::<u64>(), 0);
    }
    Ok(())
}

#[test]
fn unknown_response_quarantines_despite_cancel_then_reopen_finds_receipt()
-> Result<(), Box<dyn std::error::Error>> {
    let (catalog, spec) = build_fixture()?;
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("graph");
    let mut store = GraphDbStore::create_batched_build(&path, GraphLimits::default())?;
    store.write_facts_batched(&catalog, &ImportBudget::default(), &context())?;
    let proof = store.commit_build(&spec, &context())?;
    // Adapter-level lost-response injection after an actual successful commit.
    // This is not a native mid-commit crash simulation.
    let mut error = GraphError::local(GraphErrorKind::RecoveryRequired);
    error.outcome = Some(GraphOutcome::Unknown);
    let control = QueryControl::new()?;
    store.limits.control = Some(control.clone());
    store
        .finish::<()>(Err(error))
        .expect_err("unknown response");
    control.request(QueryStopReason::Cancelled);
    assert_eq!(
        store.counts().expect_err("quarantine").kind,
        GraphErrorKind::RecoveryRequired
    );
    drop(store);
    let reopened = GraphDbStore::open_import(&path, false, GraphLimits::default(), &context())?;
    assert_eq!(
        reopened.inspect_build(&spec, &context())?,
        Some(proof.clone())
    );
    assert_eq!(reopened.commit_build(&spec, &context())?, proof);
    Ok(())
}

#[test]
fn committed_receipt_survives_process_exit_without_checkpoint()
-> Result<(), Box<dyn std::error::Error>> {
    const KEY: &str = "ZG_G22_CONTROL_EXIT_PATH";
    if let Some(path) = std::env::var_os(KEY) {
        let (catalog, spec) = build_fixture()?;
        let store = GraphDbStore::create_batched_build(&path, GraphLimits::default())?;
        store.write_facts_batched(&catalog, &ImportBudget::default(), &context())?;
        store.commit_build(&spec, &context())?;
        // Skip destructors/checkpoint only AFTER confirmed native commit.
        std::process::exit(29);
    }
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("graph");
    let status = std::process::Command::new(std::env::current_exe()?)
        .args(["--exact", "graph::graphdb::tests::control_tests::committed_receipt_survives_process_exit_without_checkpoint", "--nocapture"])
        .env(KEY, &path).status()?;
    assert_eq!(status.code(), Some(29));
    let (_, spec) = build_fixture()?;
    let reopened = GraphDbStore::open_import(&path, false, GraphLimits::default(), &context())?;
    let proof = reopened
        .inspect_build(&spec, &context())?
        .expect("durable receipt");
    assert_eq!(reopened.commit_build(&spec, &context())?, proof);
    Ok(())
}

/// Deterministic cooperative-boundary cancellation immediately before the
/// native commit write. Facts are complete but uncommitted; a cancel must leave
/// no partial root/receipt, and a later uncanceled commit still succeeds once.
/// This covers the commit-boundary cancel window; it is not a crash inside the
/// native root/receipt transaction (that window stays a documented residual).
#[test]
fn commit_build_cancellation_before_write_leaves_no_partial_root()
-> Result<(), Box<dyn std::error::Error>> {
    let (catalog, spec) = build_fixture()?;
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("graph");
    let store = GraphDbStore::create_batched_build(&path, GraphLimits::default())?;
    store.write_facts_batched(&catalog, &ImportBudget::default(), &context())?;
    let cancelled = context();
    cancelled.cancellation.cancel();
    let error = store
        .commit_build(&spec, &cancelled)
        .expect_err("cancel before commit write");
    assert_eq!(error.kind, GraphErrorKind::Cancelled);
    drop(store);
    // No partial root/receipt: the candidate is still uncommitted and resumable.
    let reopened = GraphDbStore::open_import(&path, false, GraphLimits::default(), &context())?;
    assert_eq!(reopened.inspect_build(&spec, &context())?, None);
    let proof = reopened.commit_build(&spec, &context())?;
    assert_eq!(reopened.inspect_build(&spec, &context())?, Some(proof));
    Ok(())
}
