//! Real loopback HTTP tests with native MCP, without models or an index.

use std::{collections::HashMap, sync::Mutex};

use http::{HeaderName, HeaderValue};
use rmcp::transport::streamable_http_client::{
    StreamableHttpClient, StreamableHttpError, StreamableHttpPostResponse,
};

use super::*;
use crate::http_client::{LoopbackHttpClient, LoopbackHttpError};
use zg_engine::{
    EngineError,
    api::{index::IndexOptions, info::InfoOptions},
};
use zg_transport_mcp::IndexOperationResult;

const TOKEN: &str = "canonical-schema-http-test-token-not-a-secret";

#[derive(Default)]
struct RelationshipCalls(Mutex<Vec<RelationshipOptions>>);

#[async_trait::async_trait]
impl IndexOperationProvider for RelationshipCalls {
    async fn submit_index(
        &self,
        _: IndexOptions,
        _: bool,
    ) -> Result<IndexOperationResult, EngineError> {
        panic!("HTTP schema tests must not index")
    }

    async fn drop_index(&self, _: InfoOptions) -> Result<bool, EngineError> {
        panic!("HTTP schema tests must not drop indexes")
    }

    async fn relationships(
        &self,
        _: &ZvecGrep,
        _: Direction,
        request: RelationshipOptions,
    ) -> Result<RelationshipResult, EngineError> {
        self.0
            .lock()
            .expect("recorded HTTP calls lock")
            .push(request);
        Ok(RelationshipResult {
            matches: Vec::new(),
            suggestions: Vec::new(),
        })
    }
}

struct Server {
    base: String,
    shutdown: CancellationToken,
    task: tokio_util::task::AbortOnDropHandle<std::io::Result<()>>,
    calls: Arc<RelationshipCalls>,
}

impl Server {
    async fn start() -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("loopback test listener");
        let address = listener.local_addr().expect("listener address");
        let shutdown = CancellationToken::new();
        let engine = Arc::new(ZvecGrep::new());
        let calls = Arc::new(RelationshipCalls::default());
        let config = StreamableHttpServerConfig::default()
            .with_cancellation_token(shutdown.child_token())
            .with_allowed_hosts(["127.0.0.1".to_owned(), address.to_string()])
            .with_max_request_body_bytes(1024 * 1024);
        let app = mcp_routes(
            ZvecGrepMcpServer::build_with_index_operations(
                Arc::clone(&engine),
                McpToolset::Agent,
                None,
                calls.clone(),
            ),
            ZvecGrepMcpServer::build_with_index_operations(
                engine,
                McpToolset::Agent,
                None,
                calls.clone(),
            ),
            config,
        )
        .layer(axum::middleware::from_fn_with_state(
            Some(TOKEN.to_owned()),
            crate::authentication::authenticate,
        ));
        let signal = shutdown.clone();
        let task = tokio_util::task::AbortOnDropHandle::new(tokio::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(signal.cancelled_owned())
                .await
        }));
        Self {
            base: format!("http://{address}"),
            shutdown,
            task,
            calls,
        }
    }

    async fn post(
        &self,
        path: &str,
        mut message: Value,
        session: Option<&str>,
        version: &str,
        token: Option<&str>,
        origin: Option<&str>,
    ) -> Result<StreamableHttpPostResponse, StreamableHttpError<LoopbackHttpError>> {
        if version == "2026-07-28"
            && message.get("id").is_some()
            && message["method"] != "initialize"
        {
            // Modern requests are self-contained; mirror the SDK's request metadata.
            message["params"]["_meta"] = json!({
                "io.modelcontextprotocol/protocolVersion": version,
                "io.modelcontextprotocol/clientCapabilities": {},
                "io.modelcontextprotocol/clientInfo": {"name":"canonical-schema-offline-test","version":"1"}
            });
        }
        let mut headers = HashMap::from([(
            HeaderName::from_static("mcp-protocol-version"),
            HeaderValue::from_str(version).expect("protocol version header"),
        )]);
        if let Some(method) = message["method"].as_str() {
            headers.insert(
                HeaderName::from_static("mcp-method"),
                HeaderValue::from_str(method).expect("method header"),
            );
        }
        if let Some(name) = message["params"]["name"].as_str() {
            headers.insert(
                HeaderName::from_static("mcp-name"),
                HeaderValue::from_str(name).expect("tool name header"),
            );
        }
        if let Some(origin) = origin {
            headers.insert(
                ORIGIN,
                HeaderValue::from_str(origin).expect("origin header"),
            );
        }
        LoopbackHttpClient
            .post_message(
                format!("{}{path}", self.base).into(),
                serde_json::from_value(message).expect("valid MCP message"),
                session.map(Arc::from),
                token.map(str::to_owned),
                headers,
            )
            .await
    }

    async fn rpc(
        &self,
        path: &str,
        message: Value,
        session: Option<&str>,
        version: &str,
    ) -> (Value, Option<String>) {
        let response = self
            .post(path, message, session, version, Some(TOKEN), None)
            .await
            .expect("HTTP RPC response");
        let (message, session) = response
            .expect_initialized::<LoopbackHttpError>()
            .await
            .expect("MCP RPC response");
        (
            serde_json::to_value(message).expect("response JSON"),
            session,
        )
    }

    async fn initialize(&self, path: &str, version: &str) -> (Value, Option<String>) {
        let initialized = self.rpc(path, initialize(version), None, version).await;
        self.post(
            path,
            json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
            initialized.1.as_deref(),
            version,
            Some(TOKEN),
            None,
        )
        .await
        .expect("initialized notification response")
        .expect_accepted_or_json::<LoopbackHttpError>()
        .expect("initialized notification accepted");
        initialized
    }

    async fn stop(self) {
        self.shutdown.cancel();
        tokio::time::timeout(std::time::Duration::from_secs(5), self.task)
            .await
            .expect("both MCP services shut down")
            .expect("HTTP server task")
            .expect("HTTP server shutdown");
    }
}

fn initialize(version: &str) -> Value {
    json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{
        "protocolVersion":version,"capabilities":{},
        "clientInfo":{"name":"canonical-schema-offline-test","version":"1"}
    }})
}

fn list_tools() -> Value {
    json!({"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}})
}

fn assert_tool_schemas(legacy: &Value, canonical: &Value) {
    let legacy = legacy["result"]["tools"]
        .as_array()
        .expect("legacy tool list");
    let canonical = canonical["result"]["tools"]
        .as_array()
        .expect("canonical tool list");
    assert_eq!(legacy.len(), canonical.len());
    for (old, new) in legacy.iter().zip(canonical) {
        let mut expected = old.clone();
        if matches!(
            old["name"].as_str(),
            Some("zvec_grep_callers" | "zvec_grep_callees")
        ) {
            assert!(old["inputSchema"]["properties"]["globs"]["anyOf"].is_array());
            assert_eq!(new["inputSchema"]["properties"]["globs"]["type"], "array");
            assert!(new["inputSchema"].get("$defs").is_none());
            expected["inputSchema"] = new["inputSchema"].clone();
        }
        assert_eq!(&expected, new);
    }
}

async fn assert_relationship_calls(
    server: &Server,
    version: &str,
    legacy_session: Option<&str>,
    canonical_session: Option<&str>,
) {
    for (path, session) in [
        ("/mcp", legacy_session),
        ("/mcp/canonical", canonical_session),
    ] {
        for name in ["zvec_grep_callers", "zvec_grep_callees"] {
            for globs in [
                json!("src/example.rs"),
                json!([{"pattern":"src/example.rs"}]),
            ] {
                let before = server
                    .calls
                    .0
                    .lock()
                    .expect("recorded HTTP calls lock")
                    .len();
                let (response, _) = server
                    .rpc(
                        path,
                        json!({
                            "jsonrpc":"2.0", "id":3, "method":"tools/call",
                            "params":{"name":name,"arguments":{
                                "root":std::env::temp_dir().to_string_lossy(),
                                "symbol":"Example.update","globs":globs,"limit":20
                            }}
                        }),
                        session,
                        version,
                    )
                    .await;
                assert!(response.get("error").is_none(), "{response}");
                assert_ne!(response["result"]["isError"], true);
                let calls = server.calls.0.lock().expect("recorded HTTP calls lock");
                assert_eq!(
                    calls.len(),
                    before + 1,
                    "real HTTP tool call reached provider"
                );
                assert_eq!(
                    calls.last().expect("provider received HTTP call").globs,
                    vec!["src/example.rs".into()]
                );
            }
        }
    }
}

async fn assert_session_isolation(server: &Server, old_id: &str, new_id: &str, version: &str) {
    assert_ne!(old_id, new_id);
    for (path, wrong_id) in [("/mcp", new_id), ("/mcp/canonical", old_id)] {
        assert!(
            matches!(
                server
                    .post(
                        path,
                        list_tools(),
                        Some(wrong_id),
                        version,
                        Some(TOKEN),
                        None
                    )
                    .await,
                Err(StreamableHttpError::SessionExpired)
            ),
            "session must not cross schema endpoints"
        );
    }
    LoopbackHttpClient
        .delete_session(
            format!("{}/mcp", server.base).into(),
            old_id.into(),
            Some(TOKEN.to_owned()),
            HashMap::from([(
                HeaderName::from_static("mcp-protocol-version"),
                HeaderValue::from_static("2025-11-25"),
            )]),
        )
        .await
        .expect("delete legacy HTTP session");
    assert!(matches!(
        server
            .post(
                "/mcp",
                list_tools(),
                Some(old_id),
                version,
                Some(TOKEN),
                None
            )
            .await,
        Err(StreamableHttpError::SessionExpired)
    ));
    server
        .rpc("/mcp/canonical", list_tools(), Some(new_id), version)
        .await;
}

#[tokio::test]
async fn mcp_endpoints_select_schemas_and_isolate_legacy_sessions() {
    tokio::time::timeout(std::time::Duration::from_secs(15), async {
        let server = Server::start().await;
        for version in ["2025-11-25", "2026-07-28"] {
            let (legacy_info, legacy_session) = server.initialize("/mcp", version).await;
            let (canonical_info, canonical_session) =
                server.initialize("/mcp/canonical", version).await;
            assert_eq!(
                legacy_info["result"], canonical_info["result"],
                "same capabilities and instructions"
            );
            let (legacy, _) = server
                .rpc("/mcp", list_tools(), legacy_session.as_deref(), version)
                .await;
            let (canonical, _) = server
                .rpc(
                    "/mcp/canonical",
                    list_tools(),
                    canonical_session.as_deref(),
                    version,
                )
                .await;
            assert_tool_schemas(&legacy, &canonical);
            assert_relationship_calls(
                &server,
                version,
                legacy_session.as_deref(),
                canonical_session.as_deref(),
            )
            .await;
            if version == "2025-11-25" {
                assert_session_isolation(
                    &server,
                    legacy_session.as_deref().expect("legacy session"),
                    canonical_session.as_deref().expect("canonical session"),
                    version,
                )
                .await;
            } else {
                assert!(
                    legacy_session.is_none() && canonical_session.is_none(),
                    "modern protocol remains stateless"
                );
            }
        }
        server.stop().await;
    })
    .await
    .expect("native HTTP schema/session checks finish offline");
}

#[tokio::test]
async fn mcp_endpoints_share_authentication_and_origin_checks() {
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        let server = Server::start().await;
        for path in ["/mcp", "/mcp/canonical"] {
            let unauthorized = server
                .post(
                    path,
                    initialize("2025-11-25"),
                    None,
                    "2025-11-25",
                    None,
                    None,
                )
                .await
                .expect_err("missing token must be rejected");
            assert!(unauthorized.to_string().contains("HTTP 401"));
            let forbidden = server
                .post(
                    path,
                    initialize("2025-11-25"),
                    None,
                    "2025-11-25",
                    Some(TOKEN),
                    Some("https://untrusted.example"),
                )
                .await
                .expect_err("hostile origin must be rejected");
            assert!(forbidden.to_string().contains("HTTP 403"));
        }
        server.stop().await;
    })
    .await
    .expect("authentication checks finish offline");
}
