pub type Result<T> = std::result::Result<T, Error>;

/// Messages intentionally contain no source text, provider output, or secrets.
#[derive(Clone, Debug, thiserror::Error)]
pub enum Error {
    #[error("invalid input: {0}")]
    InvalidInput(&'static str),
    #[error("invalid provider output: {0}")]
    Provider(&'static str),
    #[error("operation cancelled")]
    Cancelled,
    #[error("operation deadline exceeded")]
    Deadline,
    #[error("resource budget exhausted")]
    ResourceExhausted,
    #[error("incompatible storage format")]
    IncompatibleFormat,
}
