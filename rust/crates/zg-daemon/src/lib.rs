//! Daemon lifecycle and loopback HTTP host.
//!
//! The daemon owns one reusable `ZvecGrep` instance. It exposes the selected agent or
//! full MCP toolset at `/mcp`, a health probe, a local shutdown endpoint, and a
//! concurrency-safe stdio bootstrap proxy.

mod authentication;
mod controller;
pub use authentication::resolve_token;
mod http_client;
mod job_scheduler;
mod mcp_sessions;
pub mod rolling_log;
mod runtime;
mod stdio;
mod workspace_runtime;

use std::{
    fmt,
    net::SocketAddr,
    path::{Path, PathBuf},
    str::FromStr,
    sync::Arc,
    time::Duration,
};

pub use controller::{DaemonStatus, server_status, start_server, stop_server_with_token};
pub use stdio::run_stdio_bridge;
use thiserror::Error;
use zg_engine::ErrorReport;
use zg_engine::ZvecGrep;
pub use zg_transport_mcp::McpToolset;

pub const DEFAULT_LISTEN: &str = "127.0.0.1:7999";
const WATCHER_IDLE_TIMEOUT_SECONDS_ENV: &str = "ZVEC_GREP_WATCHER_IDLE_TIMEOUT_SECONDS";
const MAX_TIMER_DELAY_SECONDS: u64 = 2_147_483_647 / 1_000;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ListenAddress {
    socket: SocketAddr,
}

impl ListenAddress {
    #[must_use]
    pub fn socket_addr(&self) -> SocketAddr {
        self.socket
    }

    #[must_use]
    pub fn server_url(&self) -> String {
        format!("http://{}/mcp", self.socket)
    }
}

impl Default for ListenAddress {
    fn default() -> Self {
        Self::from_str(DEFAULT_LISTEN).expect("the default daemon address must be valid")
    }
}

impl fmt::Display for ListenAddress {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.socket)
    }
}

impl FromStr for ListenAddress {
    type Err = DaemonError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let value = value.trim();
        let normalized = value
            .split_once(':')
            .filter(|(host, _)| host.eq_ignore_ascii_case("localhost"))
            .map_or_else(|| value.to_owned(), |(_, port)| format!("127.0.0.1:{port}"));
        let socket = normalized
            .parse::<SocketAddr>()
            .map_err(|_| DaemonError::InvalidListen(value.to_owned()))?;
        if !socket.ip().is_loopback() || socket.port() == 0 {
            return Err(DaemonError::InvalidListen(value.to_owned()));
        }
        Ok(Self { socket })
    }
}

#[derive(Clone, Debug)]
pub struct ServerConfig {
    pub listen: ListenAddress,
    pub home: PathBuf,
    /// None reuses an existing daemon profile and defaults new daemons to agent.
    pub mcp_toolset: Option<McpToolset>,
    pub token_file: Option<PathBuf>,
}

impl ServerConfig {
    #[must_use]
    pub const fn new(listen: ListenAddress, home: PathBuf) -> Self {
        Self {
            listen,
            home,
            mcp_toolset: None,
            token_file: None,
        }
    }
}

#[derive(Debug, Error)]
pub enum DaemonError {
    #[error(transparent)]
    Engine(#[from] zg_engine::EngineError),
    #[error("invalid daemon listen address {0:?}; expected a loopback host and non-zero port")]
    InvalidListen(String),
    #[error("cannot determine zvec-grep home; pass --home or set ZVEC_GREP_HOME")]
    MissingHome,
    #[error("zvec-grep server is already running with PID {pid}")]
    AlreadyRunning { pid: u32 },
    #[error(
        "legacy zvec-grep daemon (PID {pid}) has no daemonProtocolVersion and cannot serve this client; run `zg --server on` with the same --home to replace it"
    )]
    LegacyDaemon { pid: u32 },
    #[error(
        "zvec-grep daemon (PID {pid}) uses daemonProtocolVersion {actual}, but this client requires {expected}; use a compatible client or explicitly stop the daemon before restarting"
    )]
    ProtocolMismatch {
        pid: u32,
        actual: u32,
        expected: u32,
    },
    #[error("server shutdown request failed with HTTP {status}; daemon was not terminated")]
    ShutdownRejected { status: u16 },
    #[error(
        "zvec-grep server is already running with MCP toolset {active:?}; run `zg --server off` before changing toolsets"
    )]
    ToolsetMismatch { active: String },
    #[error("server address {0} is already in use")]
    AddressInUse(String),
    #[error("timed out waiting for zvec-grep server to {action}")]
    Timeout { action: &'static str },
    #[error("daemon process exited before becoming ready; see {log_path}")]
    ChildExited { log_path: PathBuf },
    #[error("refusing to stop the current process as a daemon")]
    RefuseCurrentProcess,
    #[error("zvec-grep server instance changed while stopping; refusing to signal PID {pid}")]
    InstanceChanged { pid: u32 },
    #[error(
        "cannot verify daemon process identity for PID {pid}: the OS start time does not match the instance record; refusing to stop the process"
    )]
    ProcessIdentityMismatch { pid: u32 },
    #[error("cannot identify a unique healthy daemon at {address}; no shutdown request was sent")]
    UnverifiedEndpoint { address: String },
    #[error("invalid daemon instance record at {0}")]
    InvalidRecord(PathBuf),
    #[error(
        "ZVEC_GREP_WATCHER_IDLE_TIMEOUT_SECONDS must be an integer between 0 and 2147483; 0 disables idle watcher eviction"
    )]
    InvalidWatcherIdleTimeout,
    #[error("daemon I/O failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("daemon state serialization failed: {0}")]
    Json(#[from] serde_json::Error),
    #[error("daemon MCP stdio bridge failed: {0}")]
    McpBridge(String),
    #[error("resident daemon is not ready")]
    NotReady,
    #[error("resident daemon request failed:\n{report}\nretryable: {retryable}")]
    Remote {
        report: Box<ErrorReport>,
        retryable: bool,
    },
}

/// Resolves the daemon home without changing process-global state.
///
/// # Errors
///
/// Returns [`DaemonError::MissingHome`] when neither an explicit value nor a
/// platform home environment variable is available.
pub fn resolve_home(explicit: Option<PathBuf>) -> Result<PathBuf, DaemonError> {
    if let Some(path) = explicit {
        return Ok(path);
    }
    if let Some(path) = std::env::var_os("ZVEC_GREP_HOME") {
        return Ok(PathBuf::from(path));
    }
    let user_home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"));
    user_home
        .map(|path| PathBuf::from(path).join(".zvec-grep"))
        .ok_or(DaemonError::MissingHome)
}

/// Executes one typed engine command through the resident daemon's local
/// administration endpoint. The command and reply are the same DTOs used by
/// direct mode, which keeps CLI behavior independent of the MCP tool profile.
///
/// # Errors
///
/// Returns [`DaemonError`] when the daemon is unavailable, the loopback HTTP
/// exchange fails, the reply is malformed, or the remote engine call fails.
pub async fn execute_command(
    home: &std::path::Path,
    command: zg_daemon_protocol::DaemonCommand,
) -> Result<zg_daemon_protocol::DaemonReply, DaemonError> {
    if let zg_daemon_protocol::DaemonCommand::Explore(options) = &command {
        options.validate()?;
        return within_graph_request(
            options.execution.request_timeout_ms,
            options.signal.clone(),
            execute_command_inner(home, command),
        )
        .await;
    }
    if let zg_daemon_protocol::DaemonCommand::Context(options)
    | zg_daemon_protocol::DaemonCommand::QueryAuthorization(options) = &command
        && !options.graph.is_off()
    {
        options
            .graph
            .validate()
            .map_err(zg_engine::EngineError::from)?;
        let execution = options.graph_execution.unwrap_or_default();
        execution.validate().map_err(zg_engine::EngineError::from)?;
        return within_graph_request(
            execution.request_timeout_ms,
            options.signal.clone(),
            execute_command_inner(home, command),
        )
        .await;
    }
    execute_command_inner(home, command).await
}

async fn within_graph_request<T>(
    timeout_ms: u64,
    signal: Option<tokio_util::sync::CancellationToken>,
    operation: impl std::future::Future<Output = Result<T, DaemonError>>,
) -> Result<T, DaemonError> {
    let signal = signal.unwrap_or_default();
    tokio::select! {
        biased;
        () = signal.cancelled() => Err(zg_engine::EngineError::cancelled("graph query cancelled during daemon dispatch").into()),
        result = tokio::time::timeout(Duration::from_millis(timeout_ms), operation) => {
            result.map_err(|_| zg_engine::EngineError::deadline_exceeded("graph query deadline exceeded during daemon dispatch"))?
        }
    }
}

async fn execute_command_inner(
    home: &std::path::Path,
    mut command: zg_daemon_protocol::DaemonCommand,
) -> Result<zg_daemon_protocol::DaemonReply, DaemonError> {
    let started = std::time::Instant::now();
    let status = server_status(home).await?;
    if !status.ready {
        return Err(DaemonError::NotReady);
    }
    let mut url = status.server_url.ok_or(DaemonError::NotReady)?;
    if let Some(prefix) = url.strip_suffix("/mcp") {
        url = format!("{prefix}/admin/execute");
    } else {
        return Err(DaemonError::McpBridge(
            "daemon published an invalid server URL".to_owned(),
        ));
    }
    let capability = match &command {
        zg_daemon_protocol::DaemonCommand::Explore(options) => Some(
            if options.request.domain == zg_engine::api::explore::QueryDomain::Semantic {
                "explore.semantic.v1"
            } else {
                "explore.code.v1"
            },
        ),
        zg_daemon_protocol::DaemonCommand::Context(options)
        | zg_daemon_protocol::DaemonCommand::QueryAuthorization(options)
            if !options.graph.is_off() =>
        {
            Some(match options.graph {
                zg_engine::api::context::graph::GraphQueryMode::Constrain { .. } => {
                    "constrain.code.v1"
                }
                _ => "augment.code.v1",
            })
        }
        _ => None,
    };
    if let Some(capability) = capability {
        let health = url
            .strip_suffix("/admin/execute")
            .ok_or_else(|| DaemonError::McpBridge("invalid daemon URL".into()))?;
        let capabilities: serde_json::Value =
            serde_json::from_slice(&http_client::get_json(&format!("{health}/healthz")).await?)?;
        require_query_capability(&capabilities, capability)?;
        let elapsed = started.elapsed().as_millis().try_into().unwrap_or(u64::MAX);
        match &mut command {
            zg_daemon_protocol::DaemonCommand::Explore(options) => {
                shrink_execution(&mut options.execution, elapsed)?;
            }
            zg_daemon_protocol::DaemonCommand::Context(options)
            | zg_daemon_protocol::DaemonCommand::QueryAuthorization(options) => {
                let mut execution = options.graph_execution.unwrap_or_default();
                shrink_execution(&mut execution, elapsed)?;
                options.graph_execution = Some(execution);
            }
            _ => {}
        }
    }
    let body = serde_json::to_vec(&command)?;
    let reply = http_client::post_json(&url, body).await?;
    match serde_json::from_slice::<zg_daemon_protocol::ExecutionResult>(&reply)? {
        zg_daemon_protocol::ExecutionResult::Success(reply) => Ok(reply),
        zg_daemon_protocol::ExecutionResult::Failure(error) => Err(DaemonError::Remote {
            report: Box::new(error.report),
            retryable: error.retryable,
        }),
    }
}

fn require_query_capability(
    capabilities: &serde_json::Value,
    capability: &str,
) -> Result<(), DaemonError> {
    if capabilities["protocol_version"] != zg_daemon_protocol::CURRENT_DAEMON_PROTOCOL_VERSION
        || !capabilities["capabilities"]
            .as_array()
            .is_some_and(|v| v.iter().any(|s| s == capability))
    {
        return Err(DaemonError::McpBridge(format!(
            "daemon does not support protocol {} {capability}; restart with a compatible graph-enabled binary",
            zg_daemon_protocol::CURRENT_DAEMON_PROTOCOL_VERSION
        )));
    }
    Ok(())
}

fn shrink_execution(
    execution: &mut zg_engine::api::explore::ExecutionOptions,
    elapsed: u64,
) -> Result<(), DaemonError> {
    execution.request_timeout_ms = execution
        .request_timeout_ms
        .checked_sub(elapsed)
        .filter(|n| *n > 0)
        .ok_or_else(|| {
            zg_engine::EngineError::deadline_exceeded(
                "graph query deadline exceeded before dispatch",
            )
        })?;
    execution.graph_timeout_ms = execution.graph_timeout_ms.min(execution.request_timeout_ms);
    Ok(())
}

/// Executes an index operation while forwarding live progress snapshots.
/// # Errors
/// Returns connection, protocol, or indexing errors, including a truncated stream.
pub async fn index_with_progress(
    home: &std::path::Path,
    request: zg_engine::api::index::IndexOptions,
    reporter: &zg_engine::api::index::progress::IndexProgressReporter,
) -> Result<zg_engine::api::index::IndexResult, DaemonError> {
    let url = admin_url(home, "index").await?;
    let result = http_client::post_index_stream(
        &url,
        serde_json::to_vec(&request)?,
        reporter,
        request.signal.as_ref(),
    )
    .await?;
    match result {
        zg_daemon_protocol::ExecutionResult::Success(zg_daemon_protocol::DaemonReply::Index(
            result,
        )) => Ok(*result),
        zg_daemon_protocol::ExecutionResult::Success(_) => Err(DaemonError::McpBridge(
            "daemon returned a reply other than index".into(),
        )),
        zg_daemon_protocol::ExecutionResult::Failure(error) => Err(DaemonError::Remote {
            report: Box::new(error.report),
            retryable: error.retryable,
        }),
    }
}

async fn admin_url(home: &Path, endpoint: &str) -> Result<String, DaemonError> {
    let status = server_status(home).await?;
    if !status.ready {
        return Err(DaemonError::NotReady);
    }
    let url = status.server_url.ok_or(DaemonError::NotReady)?;
    let prefix = url
        .strip_suffix("/mcp")
        .ok_or_else(|| DaemonError::McpBridge("daemon published an invalid server URL".into()))?;
    Ok(format!("{prefix}/admin/{endpoint}"))
}

fn configured_watcher_idle_timeout() -> Result<Option<Duration>, DaemonError> {
    configured_watcher_idle_timeout_value(std::env::var_os(WATCHER_IDLE_TIMEOUT_SECONDS_ENV))
}

fn configured_watcher_idle_timeout_value(
    configured: Option<std::ffi::OsString>,
) -> Result<Option<Duration>, DaemonError> {
    let Some(configured) = configured else {
        return Ok(Some(
            workspace_runtime::WorkspaceRuntimeManager::DEFAULT_IDLE_TTL,
        ));
    };
    let configured = configured
        .to_str()
        .ok_or(DaemonError::InvalidWatcherIdleTimeout)?
        .trim();
    if configured.is_empty() {
        return Ok(Some(
            workspace_runtime::WorkspaceRuntimeManager::DEFAULT_IDLE_TTL,
        ));
    }
    if !configured.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(DaemonError::InvalidWatcherIdleTimeout);
    }
    let seconds = configured
        .parse::<u64>()
        .map_err(|_| DaemonError::InvalidWatcherIdleTimeout)?;
    if seconds > MAX_TIMER_DELAY_SECONDS {
        return Err(DaemonError::InvalidWatcherIdleTimeout);
    }
    Ok((seconds != 0).then(|| Duration::from_secs(seconds)))
}

/// Runs the daemon and initializes its logging after acquiring the instance record.
/// The daemon runs until local shutdown or an OS termination signal, then
/// releases its instance record.
///
/// # Errors
///
/// Returns lifecycle, logging, bind, state-file, or HTTP server errors.
pub async fn run_server_with_logging(
    config: ServerConfig,
    engine: Arc<ZvecGrep>,
    init_logging: impl FnOnce(&Path) -> Result<(), DaemonError>,
) -> Result<(), DaemonError> {
    runtime::run_server(config, engine, init_logging).await
}

#[must_use]
pub const fn default_stop_timeout() -> Duration {
    Duration::from_secs(30)
}

#[cfg(test)]
mod tests {
    use std::{ffi::OsString, str::FromStr, time::Duration};

    use super::{
        ListenAddress, configured_watcher_idle_timeout_value,
        workspace_runtime::WorkspaceRuntimeManager,
    };

    #[tokio::test(start_paused = true)]
    async fn explore_dispatch_timeout_and_cancellation_keep_engine_error_codes() {
        use std::future::pending;
        use tokio_util::sync::CancellationToken;
        let error = super::within_graph_request::<()>(1, None, pending())
            .await
            .expect_err("deadline");
        assert!(
            matches!(error,super::DaemonError::Engine(ref e) if e.code()==zg_engine::EngineError::DEADLINE_EXCEEDED)
        );
        let token = CancellationToken::new();
        let cancel = token.clone();
        let operation = async move {
            cancel.cancel();
            pending::<Result<(), super::DaemonError>>().await
        };
        let error = super::within_graph_request(30_000, Some(token), operation)
            .await
            .expect_err("cancel during dispatch");
        assert!(
            matches!(error,super::DaemonError::Engine(ref e) if e.code()==zg_engine::EngineError::CANCELLED)
        );
        let token = CancellationToken::new();
        token.cancel();
        let work = std::future::poll_fn::<Result<(), super::DaemonError>, _>(|_| {
            panic!("cancelled dispatch must not poll work")
        });
        let error = super::within_graph_request(1, Some(token), work)
            .await
            .expect_err("pre-cancelled");
        assert!(
            matches!(error,super::DaemonError::Engine(ref e) if e.code()==zg_engine::EngineError::CANCELLED)
        );
    }

    #[test]
    fn explore_requires_current_protocol_and_explicit_capability() {
        for value in [
            serde_json::json!({"status":"ok"}),
            serde_json::json!({"protocol_version":12,"capabilities":["explore.code.v1"]}),
            serde_json::json!({"protocol_version":13,"capabilities":[]}),
            serde_json::json!({"protocol_version":14,"capabilities":["explore.code.v1","explore.semantic.v1"]}),
        ] {
            assert!(super::require_query_capability(&value, "explore.code.v1").is_err());
        }
        assert!(
            super::require_query_capability(
                &serde_json::json!({
                    "protocol_version":17,"capabilities":["explore.code.v1"]
                }),
                "explore.code.v1"
            )
            .is_ok()
        );
    }

    #[test]
    fn augmentation_requires_current_protocol_and_its_own_capability() {
        for value in [
            serde_json::json!({"protocol_version":13,"capabilities":["augment.code.v1"]}),
            serde_json::json!({"protocol_version":17,"capabilities":["explore.code.v1"]}),
        ] {
            assert!(super::require_query_capability(&value, "augment.code.v1").is_err());
        }
        assert!(
            super::require_query_capability(
                &serde_json::json!({"protocol_version":17,"capabilities":["augment.code.v1"]}),
                "augment.code.v1"
            )
            .is_ok()
        );
    }

    #[test]
    fn listen_address_accepts_only_loopback() {
        assert!(ListenAddress::from_str("127.0.0.1:7999").is_ok());
        assert!(ListenAddress::from_str("localhost:7999").is_ok());
        assert!(ListenAddress::from_str("LOCALHOST:7999").is_ok());
        assert!(ListenAddress::from_str("[::1]:7999").is_ok());
        assert!(ListenAddress::from_str("0.0.0.0:7999").is_err());
        assert!(ListenAddress::from_str("127.0.0.1:0").is_err());
    }

    #[test]
    fn watcher_idle_timeout_matches_node_environment_semantics() {
        assert_eq!(
            configured_watcher_idle_timeout_value(None).expect("default timeout"),
            Some(WorkspaceRuntimeManager::DEFAULT_IDLE_TTL)
        );
        assert_eq!(
            configured_watcher_idle_timeout_value(Some(OsString::from(" 15 ")))
                .expect("configured timeout"),
            Some(Duration::from_secs(15))
        );
        assert_eq!(
            configured_watcher_idle_timeout_value(Some(OsString::from("0")))
                .expect("disabled timeout"),
            None
        );
        for invalid in ["-1", "1.5", "2147484", "seconds"] {
            assert!(
                configured_watcher_idle_timeout_value(Some(OsString::from(invalid))).is_err(),
                "{invalid} must be rejected"
            );
        }
    }
}

#[cfg(test)]
mod constrain_capability_tests {
    #[test]
    fn constrain_requires_its_own_declared_capability() {
        let old = serde_json::json!({"protocol_version":15,"capabilities":["explore.code.v1","augment.code.v1"]});
        assert!(super::require_query_capability(&old, "constrain.code.v1").is_err());
        let current =
            serde_json::json!({"protocol_version":17,"capabilities":["constrain.code.v1"]});
        assert!(super::require_query_capability(&current, "constrain.code.v1").is_ok());
    }
}
