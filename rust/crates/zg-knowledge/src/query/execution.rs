use super::{GraphQueryMode, QueryError, QueryErrorKind, QueryResult};
use serde::{Deserialize, Serialize};
use std::{
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tokio_util::sync::CancellationToken;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct BudgetLimits {
    pub nodes: u64,
    pub edges: u64,
    pub candidates: u64,
    pub mapping_units: u64,
    pub output_bytes: u64,
    pub filter_bytes: u64,
}
impl Default for BudgetLimits {
    fn default() -> Self {
        Self {
            nodes: 2000,
            edges: 10_000,
            candidates: 5000,
            mapping_units: 5000,
            output_bytes: 65_536,
            filter_bytes: 262_144,
        }
    }
}
impl BudgetLimits {
    pub fn validate(self) -> QueryResult<()> {
        if self.values().contains(&0) {
            return Err(QueryError::invalid("budget limits must be positive"));
        }
        Ok(())
    }
    fn values(self) -> [u64; 6] {
        [
            self.nodes,
            self.edges,
            self.candidates,
            self.mapping_units,
            self.output_bytes,
            self.filter_bytes,
        ]
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct BudgetUsage {
    pub nodes: u64,
    pub edges: u64,
    pub candidates: u64,
    pub mapping_units: u64,
    pub output_bytes: u64,
    pub filter_bytes: u64,
}
impl BudgetUsage {
    fn values(self) -> [u64; 6] {
        [
            self.nodes,
            self.edges,
            self.candidates,
            self.mapping_units,
            self.output_bytes,
            self.filter_bytes,
        ]
    }
    fn from_values(v: [u64; 6]) -> Self {
        Self {
            nodes: v[0],
            edges: v[1],
            candidates: v[2],
            mapping_units: v[3],
            output_bytes: v[4],
            filter_bytes: v[5],
        }
    }
}

/// Clones and child stages share one ledger, cancellation signal and root deadline.
/// Creating a new query group or stage cannot replenish the total budget.
#[derive(Clone, Debug)]
pub struct RequestContext {
    cancellation: CancellationToken,
    request_deadline: Instant,
    stage_deadline: Instant,
    limits: BudgetLimits,
    usage: Arc<Mutex<BudgetUsage>>,
}
impl RequestContext {
    pub fn new(
        limits: BudgetLimits,
        cancellation: CancellationToken,
        deadline: Instant,
    ) -> QueryResult<Self> {
        limits.validate()?;
        Ok(Self {
            cancellation,
            request_deadline: deadline,
            stage_deadline: deadline,
            limits,
            usage: Arc::new(Mutex::new(BudgetUsage::default())),
        })
    }
    pub fn child(&self, timeout: Duration) -> QueryResult<Self> {
        self.check()?;
        if timeout.is_zero() {
            return Err(QueryError::invalid("stage timeout must be positive"));
        }
        let deadline = Instant::now()
            .checked_add(timeout)
            .ok_or_else(|| QueryError::invalid("stage timeout overflow"))?;
        Ok(Self {
            stage_deadline: self.stage_deadline.min(deadline),
            ..self.clone()
        })
    }
    pub fn check(&self) -> QueryResult<()> {
        let now = Instant::now();
        self.check_request_at(now)?;
        if now >= self.stage_deadline {
            return Err(QueryError::new(
                QueryErrorKind::StageDeadline,
                "execute",
                "graph stage deadline exceeded",
            ));
        }
        Ok(())
    }
    fn check_request(&self) -> QueryResult<()> {
        self.check_request_at(Instant::now())
    }
    fn check_request_at(&self, now: Instant) -> QueryResult<()> {
        if self.cancellation.is_cancelled() {
            return Err(QueryError::new(
                QueryErrorKind::Cancelled,
                "execute",
                "request cancelled",
            ));
        }
        if now >= self.request_deadline {
            return Err(QueryError::new(
                QueryErrorKind::RequestDeadline,
                "execute",
                "request deadline exceeded",
            ));
        }
        Ok(())
    }
    /// Charge all counters atomically. A rejected charge changes none of them.
    pub fn charge(&self, additional: BudgetUsage) -> QueryResult<()> {
        self.check()?;
        let mut usage = self.usage.lock().map_err(|_| {
            QueryError::new(
                QueryErrorKind::Internal,
                "budget",
                "budget ledger unavailable",
            )
        })?;
        self.check()?;
        let mut next = [0; 6];
        for (i, (used, extra)) in usage
            .values()
            .into_iter()
            .zip(additional.values())
            .enumerate()
        {
            next[i] = used
                .checked_add(extra)
                .filter(|n| *n <= self.limits.values()[i])
                .ok_or_else(|| {
                    QueryError::new(
                        QueryErrorKind::ResourceExhausted,
                        "budget",
                        "shared query budget exhausted",
                    )
                })?;
        }
        *usage = BudgetUsage::from_values(next);
        Ok(())
    }
    #[must_use]
    pub const fn limits(&self) -> BudgetLimits {
        self.limits
    }
    pub fn usage(&self) -> QueryResult<BudgetUsage> {
        self.usage.lock().map(|u| *u).map_err(|_| {
            QueryError::new(
                QueryErrorKind::Internal,
                "budget",
                "budget ledger unavailable",
            )
        })
    }
    #[must_use]
    pub fn deadline(&self) -> Instant {
        self.stage_deadline
    }

    /// Only optional augmentation may degrade; a request stop always wins.
    #[must_use]
    pub fn may_degrade(&self, mode: &GraphQueryMode, text_safe: bool, error: &QueryError) -> bool {
        text_safe
            && matches!(mode, GraphQueryMode::Augment { .. })
            && self.check_request().is_ok()
            && matches!(
                error.kind,
                QueryErrorKind::GraphUnavailable
                    | QueryErrorKind::Unsupported
                    | QueryErrorKind::ResourceExhausted
                    | QueryErrorKind::StageDeadline
                    | QueryErrorKind::ViewMismatch
            )
    }
}

/// Wire limits are validated before a host creates a runtime context.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct ExecutionOptions {
    pub request_timeout_ms: u64,
    pub graph_timeout_ms: u64,
    pub budget: BudgetLimits,
}
impl Default for ExecutionOptions {
    fn default() -> Self {
        Self {
            request_timeout_ms: 30_000,
            graph_timeout_ms: 5000,
            budget: BudgetLimits::default(),
        }
    }
}
impl ExecutionOptions {
    pub fn validate_within(self, ceiling: Self) -> QueryResult<()> {
        self.budget.validate()?;
        ceiling.budget.validate()?;
        if self.request_timeout_ms == 0
            || self.graph_timeout_ms == 0
            || self.request_timeout_ms > ceiling.request_timeout_ms
            || self.graph_timeout_ms > ceiling.graph_timeout_ms
            || self
                .budget
                .values()
                .into_iter()
                .zip(ceiling.budget.values())
                .any(|(n, max)| n > max)
        {
            return Err(QueryError::invalid(
                "execution limits exceed service allowance",
            ));
        }
        Ok(())
    }
}

impl RequestContext {
    /// A worker may cancel its own bridge token without cancelling siblings.
    #[must_use]
    pub fn worker_cancellation(&self) -> CancellationToken {
        self.cancellation.child_token()
    }
}
