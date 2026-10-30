//! Feature-gated candidate fact storage. This is not a published graph or a
//! generic database abstraction. Native handles never leave this module.
pub mod build;
mod control;
mod graphdb;
#[doc(hidden)]
pub mod profile;
pub mod schema;

pub use control::{QueryControl, QueryStopReason};
pub use graphdb::{GraphDbStore, GraphLimits, ImportBudget, ImportProgress, StoreCounts};

pub type GraphResult<T> = std::result::Result<T, GraphError>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GraphErrorKind {
    InvalidInput,
    IncompatibleFormat,
    ReadOnly,
    Conflict,
    ResourceExhausted,
    Deadline,
    Cancelled,
    RecoveryRequired,
    Storage,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GraphOutcome {
    NotStarted,
    RolledBack,
    InTransaction,
    Committed,
    Unknown,
}
/// Own codes, never native message/source text. Execution outcome is orthogonal
/// to error kind (for example a committed result-conversion failure).
#[derive(Clone, Debug, thiserror::Error)]
#[error("graph operation failed ({kind:?}, {outcome:?}, {stage})")]
pub struct GraphError {
    pub kind: GraphErrorKind,
    pub outcome: Option<GraphOutcome>,
    pub stage: String,
    pub native_kind: Option<String>,
    pub transaction_code: Option<String>,
    pub mutation_code: Option<String>,
    pub cleanup: Option<Box<Self>>,
    pub cause: Option<Box<Self>>,
}
impl GraphError {
    pub(super) fn local(kind: GraphErrorKind) -> Self {
        Self {
            kind,
            outcome: None,
            stage: "input".into(),
            native_kind: None,
            transaction_code: None,
            mutation_code: None,
            cleanup: None,
            cause: None,
        }
    }
    pub(super) fn native(error: &::graphdb::Error) -> Self {
        use ::graphdb::{
            ErrorKind as N, ExecutionOutcome as O, MutationErrorCode as M,
            TransactionErrorCode as T,
        };
        let outcome = error.execution_outcome().map(|o| match o {
            O::NotStarted => GraphOutcome::NotStarted,
            O::RolledBack => GraphOutcome::RolledBack,
            O::InTransaction { .. } => GraphOutcome::InTransaction,
            O::Committed => GraphOutcome::Committed,
            _ => GraphOutcome::Unknown,
        });
        let kind = if outcome == Some(GraphOutcome::Unknown)
            || error.kind() == N::CommitOutcomeUnknown
            || matches!(
                error.transaction_code(),
                Some(T::Unusable | T::CleanupFailed)
            )
            || error.cleanup_error().is_some()
        {
            GraphErrorKind::RecoveryRequired
        } else if error.transaction_code() == Some(T::ReadOnly)
            || error.mutation_code() == Some(M::ReadOnly)
        {
            GraphErrorKind::ReadOnly
        } else {
            match error.kind() {
                N::ResourceExhausted => GraphErrorKind::ResourceExhausted,
                N::Cancelled => GraphErrorKind::Cancelled,
                N::AdmissionTimeout => GraphErrorKind::Deadline,
                N::InvalidArgument | N::OutOfRange => GraphErrorKind::InvalidInput,
                _ if matches!(
                    error.mutation_code(),
                    Some(M::DuplicateKey | M::WriteConflict)
                ) =>
                {
                    GraphErrorKind::Conflict
                }
                _ => GraphErrorKind::Storage,
            }
        };
        Self {
            kind,
            outcome,
            stage: format!("{:?}", error.stage()),
            native_kind: Some(format!("{:?}", error.kind())),
            transaction_code: error.transaction_code().map(|c| format!("{c:?}")),
            mutation_code: error.mutation_code().map(|c| format!("{c:?}")),
            cleanup: error.cleanup_error().map(|e| Box::new(Self::native(e))),
            cause: error.cause().map(|e| Box::new(Self::native(e))),
        }
    }
}
impl From<crate::Error> for GraphError {
    fn from(error: crate::Error) -> Self {
        Self::local(match error {
            crate::Error::Cancelled => GraphErrorKind::Cancelled,
            crate::Error::Deadline => GraphErrorKind::Deadline,
            crate::Error::ResourceExhausted => GraphErrorKind::ResourceExhausted,
            crate::Error::IncompatibleFormat => GraphErrorKind::IncompatibleFormat,
            _ => GraphErrorKind::InvalidInput,
        })
    }
}
impl From<::graphdb::Error> for GraphError {
    fn from(error: ::graphdb::Error) -> Self {
        Self::native(&error)
    }
}
impl From<std::io::Error> for GraphError {
    fn from(_: std::io::Error) -> Self {
        Self::local(GraphErrorKind::Storage)
    }
}
