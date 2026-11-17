use std::sync::Mutex;

use rmcp::{ServiceExt, model::ErrorCode};
use serde_json::{Value, json};

use super::*;

struct Status;
impl ServerStatusProvider for Status {
    fn snapshot(&self) -> ServerStatusSnapshot {
        ServerStatusSnapshot::default()
    }
}

fn server(toolset: McpToolset) -> ZvecGrepMcpServer {
    ZvecGrepMcpServer::build(Arc::new(ZvecGrep::new()), toolset, Some(Arc::new(Status)))
}

#[test]
fn canonical_view_changes_only_relationship_schemas_in_all_toolsets() {
    for toolset in [
        McpToolset::Agent,
        McpToolset::Full,
        McpToolset::Search,
        McpToolset::Relationships,
    ] {
        let legacy = server(toolset);
        let before = legacy.listed_tools();
        let canonical = legacy.clone().with_canonical_relationship_schema();
        assert_eq!(
            before,
            legacy.listed_tools(),
            "clone must not mutate legacy"
        );
        assert_eq!(legacy.get_info(), canonical.get_info());
        let after = canonical.listed_tools();
        assert_eq!(before.len(), after.len());
        for (old, new) in before.iter().zip(&after) {
            let mut expected = old.clone();
            if matches!(old.name.as_ref(), "zvec_grep_callers" | "zvec_grep_callees") {
                // Default construction must keep rmcp's original generated schema.
                assert_eq!(
                    old.input_schema,
                    ZvecGrepMcpServer::tool_router().map[&old.name]
                        .attr
                        .input_schema
                );
                expected.input_schema = new.input_schema.clone();
                let value = Value::Object((*new.input_schema).clone());
                assert!(value.get("$defs").is_none());
                for key in ["$ref", "anyOf", "oneOf"] {
                    assert!(!value.to_string().contains(&format!("\"{key}\"")));
                }
                assert_eq!(value["required"], json!(["root", "symbol"]));
                for field in ["root", "symbol", "limit"] {
                    assert_eq!(
                        value["properties"][field],
                        old.input_schema["properties"][field]
                    );
                }
                assert_eq!(value["properties"]["globs"]["type"], "array");
                assert_eq!(value["properties"]["globs"]["maxItems"], MAX_PATH_FILTERS);
            }
            assert_eq!(
                expected, *new,
                "{} metadata or non-relationship schema changed",
                old.name
            );
        }
    }
}

#[test]
fn canonical_schema_validates_rule_arrays_and_preserves_legacy_contract() {
    let legacy = server(McpToolset::Relationships);
    let canonical = legacy.clone().with_canonical_relationship_schema();
    let schema = canonical.listed_tools()[0].input_schema.clone();
    let validator = jsonschema::validator_for(&Value::Object((*schema).clone()))
        .expect("valid canonical schema");
    let legacy_validator = jsonschema::validator_for(&Value::Object(
        (*legacy.listed_tools()[0].input_schema).clone(),
    ))
    .expect("valid legacy schema");
    let body = |filters: Value| {
        let mut args = json!({"root": "/workspace", "symbol": "Subject.run"});
        args.as_object_mut()
            .expect("argument object")
            .extend(filters.as_object().expect("filter object").clone());
        args
    };
    for filters in [
        json!({}),
        json!({"globs": []}),
        json!({"globs": [{"pattern": "src/**"}, {"pattern": "!src/tests/**", "caseInsensitive": true}], "insensitiveGlobs": ["*.RS"], "limit": 20}),
        json!({"globs": vec![json!({"pattern": "a".repeat(MAX_PATH_CHARS)}); MAX_PATH_FILTERS]}),
    ] {
        assert!(validator.is_valid(&body(filters.clone())), "{filters}");
        assert!(
            legacy_validator.is_valid(&body(filters)),
            "canonical values are accepted by legacy"
        );
    }
    for filters in [
        json!({"globs": "src/**"}),
        json!({"globs": ["src/**"]}),
        json!({"globs": null}),
        json!({"insensitiveGlobs": "*.RS"}),
    ] {
        assert!(legacy_validator.is_valid(&body(filters.clone())));
        assert!(
            !validator.is_valid(&body(filters)),
            "narrowing is explicit and opt-in"
        );
    }
    for filters in [
        json!({"globs": [{"pattern": ""}]}),
        json!({"globs": [{"pattern": "a".repeat(MAX_PATH_CHARS + 1)}]}),
        json!({"globs": vec![json!({"pattern": "*.py"}); MAX_PATH_FILTERS + 1]}),
        json!({"globs": [{"caseInsensitive": true}]}),
        json!({"globs": [{"pattern": "*.py", "caseInsensitive": "true"}]}),
        json!({"globs": [{"pattern": "*.py", "extra": true}]}),
        json!({"insensitiveGlobs": null}),
        json!({"insensitiveGlobs": [true]}),
        json!({"insensitiveGlobs": vec!["*.RS"; MAX_PATH_FILTERS + 1]}),
        json!({"limit": 0}),
        json!({"limit": MAX_RELATIONSHIP_EDGES + 1}),
        json!({"unknown": 1}),
    ] {
        assert!(
            !validator.is_valid(&body(filters)),
            "invalid canonical parameters"
        );
    }
}

#[derive(Default)]
struct RecordingRelationships(Mutex<Vec<RelationshipOptions>>);

#[async_trait]
impl IndexOperationProvider for RecordingRelationships {
    async fn submit_index(
        &self,
        _: IndexOptions,
        _: bool,
    ) -> Result<IndexOperationResult, EngineError> {
        panic!("relationship compatibility test must not index")
    }

    async fn drop_index(&self, _: InfoOptions) -> Result<bool, EngineError> {
        panic!("relationship compatibility test must not drop indexes")
    }

    async fn relationships(
        &self,
        _: &ZvecGrep,
        _: Direction,
        request: RelationshipOptions,
    ) -> Result<RelationshipResult, EngineError> {
        self.0.lock().expect("recorded calls lock").push(request);
        Ok(RelationshipResult {
            matches: Vec::new(),
            suggestions: Vec::new(),
        })
    }
}

#[tokio::test]
async fn both_schema_views_preserve_native_mcp_argument_and_filter_semantics() {
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        let root = std::env::temp_dir().to_string_lossy().into_owned();
        let mut legacy_responses = Vec::new();
        for canonical in [false, true] {
            let provider = Arc::new(RecordingRelationships::default());
            let server = ZvecGrepMcpServer::build_with_index_operations(
                Arc::new(ZvecGrep::new()), McpToolset::Relationships, None, provider.clone(),
            );
            let server = if canonical { server.with_canonical_relationship_schema() } else { server };
            let (server_io, client_io) = tokio::io::duplex(16384);
            let task = tokio::spawn(async move {
                server.serve(server_io).await.expect("native MCP server").waiting().await
            });
            let client = ().serve(client_io).await.expect("native MCP client");
            let mut response_index = 0;
            for name in ["zvec_grep_callers", "zvec_grep_callees"] {
                for filters in [
                    json!({}),
                    json!({"globs": " src/** "}),
                    json!({"globs": ["src/**", "!src/tests/**"]}),
                    json!({"globs": [{"pattern": "SRC/**", "caseInsensitive": true}, {"pattern": "!SRC/tests/**"}], "insensitiveGlobs": ["*.RS"]}),
                    json!({"globs": "[\"*.py\"]"}),
                    json!({"globs": [{"pattern": " src/** "}]}),
                    json!({"globs": []}),
                    json!({"globs": "", "insensitiveGlobs": " *.RS "}),
                    json!({"globs": ["src/**", "", " "]}),
                    json!({"globs": null}),
                    json!({"insensitiveGlobs": null}),
                    json!({"limit": null}),
                    json!({"globs": true}),
                    json!({"globs": ["*.py", {"pattern": "*.rs"}]}),
                    json!({"globs": [{"pattern": ""}]}),
                    json!({"globs": [{"pattern": "*.py", "unknown": true}]}),
                    json!({"globs": vec!["*.py"; MAX_PATH_FILTERS + 1]}),
                    json!({"limit": 0}),
                    json!({"unknown": true}),
                ] {
                    let mut args = json!({"root": root, "symbol": "Subject.run", "limit": 20});
                    args.as_object_mut().expect("argument object").extend(filters.as_object().expect("filter object").clone());
                    let expected = if args.as_object().expect("argument object").values().any(Value::is_null) {
                        None // The real MCP handler rejects explicit null before serde decoding.
                    } else {
                        serde_json::from_value::<RelationshipInput>(args.clone()).ok().and_then(|x| x.into_request().ok())
                    };
                    let before = provider.0.lock().expect("recorded calls lock").len();
                    let response = client.call_tool(CallToolRequestParams::new(name)
                        .with_arguments(args.as_object().expect("argument object").clone())).await;
                    let wire = match response {
                        Ok(result) => json!({"result": result}),
                        Err(rmcp::ServiceError::McpError(error)) => {
                            assert_eq!(error.code, ErrorCode::INVALID_PARAMS);
                            json!({"error": error})
                        }
                        Err(error) => panic!("unexpected transport error: {error}"),
                    };
                    if canonical {
                        assert_eq!(wire, legacy_responses[response_index], "response changed for {args}");
                    } else {
                        legacy_responses.push(wire.clone());
                    }
                    response_index += 1;
                    if let Some(expected) = expected {
                        assert!(wire.get("result").is_some());
                        assert_ne!(wire["result"]["isError"], true);
                        let calls = provider.0.lock().expect("recorded calls lock");
                        assert_eq!(calls.len(), before + 1);
                        let actual = calls.last().expect("provider received call");
                        assert_eq!(actual.globs, expected.globs, "{args}");
                        assert_eq!(actual.symbol, expected.symbol);
                        assert_eq!(actual.limit, expected.limit);
                    } else {
                        // rmcp parameter decoding returns an error tool result;
                        // handler validation returns a protocol INVALID_PARAMS.
                        assert!(wire.get("error").is_some() || wire["result"]["isError"] == true, "{wire}");
                        assert_eq!(provider.0.lock().expect("recorded calls lock").len(), before);
                    }
                }
            }
            client.cancel().await.expect("client shutdown");
            task.await.expect("server task").expect("server shutdown");
        }
    }).await.expect("native MCP argument checks finish without models or indexes");
}
