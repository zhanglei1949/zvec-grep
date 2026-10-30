use super::*;
use crate::{
    EngineError,
    workspace::lock::{LockMode, acquire_read_write_lock},
};
use std::{
    path::PathBuf,
    sync::mpsc,
    time::{Duration, Instant},
};
use zg_knowledge::{
    graph::{GraphDbStore, GraphErrorKind},
    processing::stable_digest,
    relations::bound::FactCatalog,
};

fn neutral_context() -> ProcessingContext {
    ProcessingContext {
        max_input_bytes: 1 << 20,
        max_output_items: 100,
        deadline: None,
        cancellation: tokio_util::sync::CancellationToken::new(),
    }
}
async fn prepare(path: PathBuf) {
    tokio::task::spawn_blocking(move || {
        let ctx = neutral_context();
        let facts = FactCatalog::resolve(
            vec![],
            stable_digest(&"config").expect("graph control fixture"),
            stable_digest(&"selection").expect("graph control fixture"),
            &ctx,
        )
        .expect("graph control fixture");
        let graph =
            GraphDbStore::create(&path, GraphLimits::default()).expect("graph control fixture");
        graph
            .write_facts(&facts, &ctx)
            .expect("graph control fixture");
        graph.checkpoint().expect("graph control fixture");
    })
    .await
    .expect("prepare real graph");
}
async fn stopped(query: &QueryControl) {
    tokio::time::timeout(Duration::from_secs(10), async {
        while query.reason().is_none() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("watcher forwards stop");
}

#[tokio::test]
async fn graph_control_host_cancel_and_deadline_reach_native_read() {
    for reason in [QueryStopReason::Cancelled, QueryStopReason::Deadline] {
        let dir = tempfile::tempdir().expect("graph control fixture");
        let path = dir.path().join("graph");
        prepare(path.clone()).await;
        let admission = Arc::new(
            acquire_read_write_lock(&dir.path().join("lock"), LockMode::Write, "test")
                .expect("graph control fixture"),
        );
        let mut ctx = neutral_context();
        if reason == QueryStopReason::Deadline {
            ctx.deadline = Some(Instant::now() + Duration::from_millis(25));
        }
        let signal = ctx.cancellation.clone();
        let (entered, started) = tokio::sync::oneshot::channel();
        let (release, wait) = mpsc::channel();
        let task = tokio::spawn(run(admission, ctx, move |operation| {
            let _ = entered.send(operation.query.clone());
            wait.recv_timeout(Duration::from_secs(10))
                .expect("release worker");
            // A neutral cooperative context: only the native QueryOptions
            // token can cause this real read to fail.
            let error = GraphDbStore::open(&path, true, operation.limits())
                .err()
                .expect("native stop");
            assert_eq!(error.native_kind.as_deref(), Some("Cancelled"));
            assert_eq!(
                error.kind,
                match reason {
                    QueryStopReason::Cancelled => GraphErrorKind::Cancelled,
                    QueryStopReason::Deadline => GraphErrorKind::Deadline,
                }
            );
            Err::<(), _>(super::super::graph_error(&error))
        }));
        let query = started.await.expect("graph control fixture");
        if reason == QueryStopReason::Cancelled {
            signal.cancel();
        }
        stopped(&query).await;
        release.send(()).expect("graph control fixture");
        let error = task
            .await
            .expect("graph control fixture")
            .expect_err("operation must fail");
        assert_eq!(
            error.code(),
            match reason {
                QueryStopReason::Cancelled => EngineError::CANCELLED,
                QueryStopReason::Deadline => EngineError::DEADLINE_EXCEEDED,
            }
        );
    }
}

#[tokio::test]
async fn graph_control_caller_abort_keeps_native_bridge_and_os_admission() {
    let dir = tempfile::tempdir().expect("graph control fixture");
    let path = dir.path().join("graph");
    prepare(path.clone()).await;
    let lock_path = dir.path().join("lock");
    let admission = Arc::new(
        acquire_read_write_lock(&lock_path, LockMode::Write, "original")
            .expect("graph control fixture"),
    );
    let ctx = neutral_context();
    let signal = ctx.cancellation.clone();
    let (entered, started) = tokio::sync::oneshot::channel();
    let (done, finished) = tokio::sync::oneshot::channel();
    let (release, wait) = mpsc::channel();
    let reopen = path.clone();
    let parent = tokio::spawn(run(admission, ctx, move |operation| {
        let _ = entered.send(operation.query.clone());
        wait.recv_timeout(Duration::from_secs(10))
            .expect("release worker");
        let error = GraphDbStore::open(path, true, operation.limits())
            .err()
            .expect("native stopped");
        let _ = done.send((error.kind, error.native_kind));
        Ok(())
    }));
    let query = started.await.expect("graph control fixture");
    parent.abort();
    assert!(
        parent
            .await
            .expect_err("operation must fail")
            .is_cancelled()
    );
    let blocked = acquire_read_write_lock(&lock_path, LockMode::Write, "competitor");
    signal.cancel();
    stopped(&query).await;
    // Release before assertions, and bound every blocking gate.
    release.send(()).expect("graph control fixture");
    assert_eq!(
        blocked.expect_err("operation must fail").code(),
        EngineError::RESOURCE_BUSY
    );
    let (kind, native) = finished.await.expect("graph control fixture");
    assert_eq!(kind, GraphErrorKind::Cancelled);
    assert_eq!(native.as_deref(), Some("Cancelled"));
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if let Ok(lock) = acquire_read_write_lock(&lock_path, LockMode::Write, "after") {
                drop(lock);
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("graph control fixture");
    tokio::task::spawn_blocking(move || {
        let graph = GraphDbStore::open(reopen, true, GraphLimits::default())
            .expect("graph control fixture");
        assert_eq!(
            graph
                .read_facts(&neutral_context())
                .expect("graph control fixture")
                .files
                .len(),
            0
        );
    })
    .await
    .expect("graph control fixture");
}

#[tokio::test]
async fn graph_control_normal_finish_stops_deadline_watcher() {
    let dir = tempfile::tempdir().expect("graph control fixture");
    let admission = Arc::new(
        acquire_read_write_lock(&dir.path().join("lock"), LockMode::Write, "test")
            .expect("graph control fixture"),
    );
    let mut ctx = neutral_context();
    ctx.deadline = Some(Instant::now() + Duration::from_secs(60));
    let query = run(admission, ctx, |operation| Ok(operation.query))
        .await
        .expect("graph control fixture");
    // Let abortion/drop of the watcher complete. It must not request a stop
    // just because the completed worker dropped its watcher.
    tokio::task::yield_now().await;
    assert_eq!(query.reason(), None);
}
