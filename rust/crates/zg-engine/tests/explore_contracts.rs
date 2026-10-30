use zg_engine::{EngineError, api::explore::*};
fn options() -> ExploreOptions {
    ExploreOptions::new(ExploreRequest {
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
#[cfg(feature = "knowledge-graph")]
#[test]
fn public_request_schema_matches_the_neutral_kg_contract() {
    let wire = serde_json::to_value(options()).expect("wire");
    let request: zg_knowledge::query::ExploreRequest =
        serde_json::from_value(wire["request"].clone()).expect("neutral request");
    request.validate().expect("valid");
    let execution: zg_knowledge::query::ExecutionOptions =
        serde_json::from_value(wire["execution"].clone()).expect("neutral limits");
    execution
        .validate_within(zg_knowledge::query::ExecutionOptions::default())
        .expect("same ceilings");
}
