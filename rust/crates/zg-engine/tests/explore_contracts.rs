use zg_engine::{EngineError, api::explore::*};
fn options() -> ExploreOptions {
    ExploreOptions::new(ExploreRequest {
        page_size: 64,
        cursor: None,
        relation_filters: Vec::new(),
        domain: QueryDomain::Code,
        operation: ExploreOperation::Locate {
            target: Locator::Name {
                name: "target".into(),
                source_id: None,
                kind: None,
            },
        },
        boundary: SourceFilter::default(),
        result_filter: SourceFilter::default(),
    })
}
#[test]
fn explore_wire_rejects_refresh_models_missing_requests_and_invalid_limits() {
    for json in [
        serde_json::json!({}),
        serde_json::json!({"request":{"domain":"code","operation":{"operation":"locate","target":{"by":"name","name":"target"}}},"refresh":"wait"}),
    ] {
        assert!(serde_json::from_value::<ExploreOptions>(json).is_err());
    }
    let mut value = serde_json::to_value(options()).expect("options");
    value["embedding"] = "remote/model".into();
    assert!(serde_json::from_value::<ExploreOptions>(value).is_err());
    let mut invalid = options();
    invalid.execution.budget.nodes = 0;
    assert_eq!(
        invalid.validate().expect_err("zero budget").code(),
        EngineError::INVALID_ARGUMENT
    );
    invalid = options();
    invalid.execution.graph_timeout_ms = 5001;
    assert!(invalid.validate().is_err());
    assert!(
        serde_json::to_value(options())
            .expect("wire")
            .get("signal")
            .is_none()
    );
}
#[cfg(not(feature = "knowledge-graph-graphdb"))]
#[tokio::test]
async fn default_build_rejects_explore_before_workspace_access() {
    let engine = zg_engine::ZvecGrep::new();
    let mut opts = options();
    opts.root = Some("/path/which/does/not/exist/g31".into());
    assert_eq!(
        engine
            .explore(opts)
            .await
            .expect_err("backend absent")
            .code(),
        EngineError::UNSUPPORTED
    );
    assert_eq!(engine.runtime_snapshot().loaded_models, 0);
    engine.close();
}
#[test]
fn public_request_schema_matches_the_neutral_kg_contract() {
    let options = options();
    let request: &zg_knowledge::query::ExploreRequest = &options.request;
    request.validate().expect("valid");
    let execution: zg_knowledge::query::ExecutionOptions = options.execution;
    execution.validate().expect("same ceilings");
}

#[test]
fn current_graph_api_schemas_preserve_result_and_index_contracts() {
    use zg_engine::api::{context, explore, index};
    let actual = serde_json::json!({
        "explore_request": schemars::schema_for!(explore::ExploreRequest),
        "explore_result": schemars::schema_for!(explore::ExploreResult),
        "execution": schemars::schema_for!(explore::ExecutionOptions),
        "query_mode": schemars::schema_for!(context::graph::GraphQueryMode),
        "index_options": schemars::schema_for!(index::graph::GraphOptions),
    });
    let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/graph-api-schema-current.json");
    if std::env::var_os("UPDATE_GRAPH_API_SCHEMA").is_some() {
        std::fs::write(
            &fixture,
            serde_json::to_string_pretty(&actual).expect("schema JSON") + "\n",
        )
        .expect("write approved schema snapshot");
    }
    let expected: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(fixture).expect("current public schemas"))
            .expect("schema JSON");
    assert_eq!(actual, expected);
    let historical: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/graph-api-schema-m4-m6.json"))
            .expect("historical M4-M6 schemas");
    // Publication-bound references and paging intentionally extend result shapes.
    // Existing business records and storage options retain their original fields.
    assert_eq!(actual["index_options"], historical["index_options"]);
    for record in [
        "EntityRecord",
        "GraphNode",
        "EvidenceRead",
        "EvidenceRef",
        "GraphPath",
        "LocateResult",
    ] {
        assert_eq!(
            actual["explore_result"]["$defs"][record],
            historical["explore_result"]["$defs"][record],
            "{record}"
        );
    }
    assert_eq!(
        actual["explore_request"]["properties"]["page_size"]["default"],
        64
    );
    for reference in ["NodeRef", "ObjectRef"] {
        assert!(
            actual["explore_result"]["$defs"][reference]["required"]
                .as_array()
                .expect("required fields")
                .contains(&serde_json::json!("generation"))
        );
    }
    let previous: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/graph-api-schema-m3.json"))
            .expect("M3 historical schema");
    let mut previous_index = actual["index_options"].clone();
    previous_index["$defs"]["GraphLanguage"]["enum"] =
        previous["index_options"]["$defs"]["GraphLanguage"]["enum"].clone();
    assert_eq!(previous_index, previous["index_options"]);
}
