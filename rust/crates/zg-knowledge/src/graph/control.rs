//! Owned, thread-safe query cancellation. No connection or FFI handle escapes.
use super::{GraphError, GraphErrorKind, GraphResult};
use std::sync::{
    Arc,
    atomic::{AtomicU8, Ordering},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QueryStopReason {
    Cancelled,
    Deadline,
}

#[derive(Clone, Debug)]
pub struct QueryControl {
    token: graphdb::CancelToken,
    reason: Arc<AtomicU8>,
}
impl QueryControl {
    pub fn new() -> GraphResult<Self> {
        Ok(Self {
            token: graphdb::CancelToken::new()?,
            reason: Arc::new(AtomicU8::new(0)),
        })
    }
    /// First request wins, independently of any later caller cancellation.
    pub fn request(&self, reason: QueryStopReason) {
        let code = match reason {
            QueryStopReason::Cancelled => 1,
            QueryStopReason::Deadline => 2,
        };
        let _ = self
            .reason
            .compare_exchange(0, code, Ordering::AcqRel, Ordering::Acquire);
        self.token.cancel();
    }
    #[must_use]
    pub fn reason(&self) -> Option<QueryStopReason> {
        match self.reason.load(Ordering::Acquire) {
            1 => Some(QueryStopReason::Cancelled),
            2 => Some(QueryStopReason::Deadline),
            _ => None,
        }
    }
    pub(super) fn token(&self) -> graphdb::CancelToken {
        self.token.clone()
    }
    pub(super) fn classify(&self, mut error: GraphError) -> GraphError {
        // Never downgrade Unknown, unusable/cleanup failures or known commit
        // effects. The native category and nested cause are retained.
        if error.kind == GraphErrorKind::Cancelled
            && self.reason() == Some(QueryStopReason::Deadline)
        {
            error.kind = GraphErrorKind::Deadline;
        }
        error
    }
}
