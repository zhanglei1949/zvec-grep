//! One cancellation watcher per admitted blocking operation. The worker owns
//! it, so aborting an async caller never removes cancellation from native work.
use super::{EngineResult, GraphLimits, ProcessingContext, in_worker};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use tokio_util::sync::CancellationToken;
use zg_knowledge::graph::{QueryControl, QueryStopReason};

#[derive(Clone)]
pub(super) struct Operation {
    pub context: ProcessingContext,
    query: QueryControl,
}
impl Operation {
    pub fn limits(&self) -> GraphLimits {
        GraphLimits {
            max_rows: 4_000_000,
            // Large candidate batches can wait behind native checkpoint work.
            admission_timeout: std::time::Duration::from_secs(60),
            control: Some(self.query.clone()),
            ..GraphLimits::default()
        }
    }

    /// A committed effect outlives the caller's cancellation. Confirmation gets
    /// fresh native control and a short independent deadline, while the original
    /// context and native control remain observable as late-stop diagnostics.
    pub fn confirmation(&self) -> EngineResult<Confirmation> {
        let query = QueryControl::new().map_err(|e| super::graph_error(&e))?;
        let context = ProcessingContext {
            deadline: Some(std::time::Instant::now() + std::time::Duration::from_secs(5)),
            cancellation: CancellationToken::default(),
            ..self.context.clone()
        };
        let watch = Watch::start(&context, query.clone());
        Ok(Confirmation {
            original: self.clone(),
            operation: Self { context, query },
            _watch: watch,
        })
    }

    #[cfg(test)]
    pub fn stop_for_test(&self, reason: QueryStopReason) {
        if reason == QueryStopReason::Cancelled {
            self.context.cancellation.cancel();
        }
        self.query.request(reason);
    }
}

pub(super) struct Confirmation {
    original: Operation,
    operation: Operation,
    _watch: Watch,
}
impl Confirmation {
    pub fn context(&self) -> &ProcessingContext {
        &self.operation.context
    }

    pub fn limits(&self) -> GraphLimits {
        self.operation.limits()
    }

    pub fn original_stop(&self) -> Option<QueryStopReason> {
        self.original.query.reason().or_else(|| {
            if self.original.context.cancellation.is_cancelled() {
                Some(QueryStopReason::Cancelled)
            } else if self
                .original
                .context
                .deadline
                .is_some_and(|deadline| deadline <= std::time::Instant::now())
            {
                Some(QueryStopReason::Deadline)
            } else {
                None
            }
        })
    }
}
struct Watch {
    task: tokio::task::JoinHandle<()>,
    completed: Arc<AtomicBool>,
}
impl Drop for Watch {
    fn drop(&mut self) {
        self.completed.store(true, Ordering::Release);
        self.task.abort();
    }
}
struct WatchLife {
    completed: Arc<AtomicBool>,
    query: QueryControl,
}
impl Drop for WatchLife {
    fn drop(&mut self) {
        // Runtime shutdown can drop this future while the worker still exists.
        if !self.completed.load(Ordering::Acquire) {
            self.query.request(QueryStopReason::Cancelled);
        }
    }
}
impl Watch {
    fn start(context: &ProcessingContext, query: QueryControl) -> Self {
        let completed = Arc::new(AtomicBool::new(false));
        let life = WatchLife {
            completed: Arc::clone(&completed),
            query: query.clone(),
        };
        let signal = context.cancellation.clone();
        let deadline = context.deadline;
        // An already stopped request must not wait for the watcher to be polled.
        if signal.is_cancelled() {
            query.request(QueryStopReason::Cancelled);
        } else if deadline.is_some_and(|d| d <= std::time::Instant::now()) {
            query.request(QueryStopReason::Deadline);
        }
        let task = tokio::spawn(async move {
            let _life = life;
            let expiry = async {
                match deadline {
                    Some(d) => tokio::time::sleep_until(tokio::time::Instant::from_std(d)).await,
                    None => std::future::pending::<()>().await,
                }
            };
            tokio::select! {biased;
                ()=signal.cancelled()=>query.request(QueryStopReason::Cancelled),
                ()=expiry=>query.request(QueryStopReason::Deadline),
            }
        });
        Self { task, completed }
    }
}
pub(super) async fn run<T: Send + 'static>(
    admission: Arc<crate::workspace::lock::FileLock>,
    context: ProcessingContext,
    action: impl FnOnce(Operation) -> EngineResult<T> + Send + 'static,
) -> EngineResult<T> {
    let query = QueryControl::new().map_err(|e| super::graph_error(&e))?;
    let watch = Watch::start(&context, query.clone());
    in_worker(admission, move || {
        let _watch = watch;
        action(Operation { context, query })
    })
    .await
}

#[cfg(test)]
#[path = "control_tests.rs"]
mod tests;
