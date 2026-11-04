use serde_json::json;
use tempfile::tempdir;
use zg_engine::{
    EngineError, ZvecGrep,
    api::context::{
        ContextOptions,
        graph::{BodyRegion, GraphQueryMode, Locator, ScopeSpec, TraversalSpec},
    },
};

fn modes() -> Vec<GraphQueryMode> {
    vec![
        GraphQueryMode::Augment {
            seeds: 5,
            expansion: TraversalSpec::default(),
        },
        GraphQueryMode::Constrain {
            scope: ScopeSpec {
                anchor: Locator::Name {
                    name: "validate".into(),
                    source_id: Some("src/a.rs".into()),
                    kind: None,
                },
                traversal: TraversalSpec::default(),
                region: BodyRegion::Full,
            },
        },
    ]
}

#[test]
fn off_keeps_the_legacy_request_shape_and_rejects_hidden_constraints() {
    let value = serde_json::to_value(ContextOptions::default()).expect("request JSON");
    assert!(value.get("graph").is_none());
    let mut old = value.clone();
    let parsed: ContextOptions = serde_json::from_value(old.clone()).expect("legacy request");
    assert!(parsed.graph.is_off());
    old["graph"] = json!({"mode":"off"});
    let parsed: ContextOptions = serde_json::from_value(old).expect("explicit off");
    assert_eq!(serde_json::to_value(parsed).expect("unchanged wire"), value);
    for graph in [
        json!({"mode":"off","scope":{}}),
        json!({"mode":"constrain"}),
        json!({"mode":"augment","unknown":true}),
        json!({"mode":"constrain","scope":{"anchor":{"by":"name","name":"f"},"fragment_ids":["f"]}}),
    ] {
        let mut invalid = value.clone();
        invalid["graph"] = graph;
        assert!(serde_json::from_value::<ContextOptions>(invalid).is_err());
    }
}

#[tokio::test]
async fn graph_execution_and_authorization_reject_before_workspace_or_models() {
    let dir = tempdir().expect("tempdir");
    let root = dir.path().join("does-not-exist");
    let engine = ZvecGrep::new();
    for graph in modes() {
        for rg in [false, true] {
            if cfg!(feature = "knowledge-graph-graphdb") && !rg && !graph.is_off() {
                continue; // The enabled mode is exercised against a published native view.
            }
            let request = ContextOptions {
                query: Some("timeout".into()),
                root: Some(root.clone()),
                graph: graph.clone(),
                rg,
                ..ContextOptions::default()
            };
            let expected = if rg {
                EngineError::INVALID_ARGUMENT
            } else {
                EngineError::UNSUPPORTED
            };
            let error = zg_engine::authorization::query_authorizations(&request)
                .expect_err("authorization gate");
            assert_eq!(error.code(), expected);
            let error = engine.context(request).await.expect_err("execution gate");
            assert_eq!(error.code(), expected);
            assert!(!root.exists());
            assert_eq!(engine.runtime_snapshot().loaded_models, 0);
        }
    }
    engine.close();
}

#[tokio::test]
async fn invalid_graph_parameters_fail_before_the_capability_gate() {
    let error = ZvecGrep::new()
        .context(ContextOptions {
            graph: GraphQueryMode::Augment {
                seeds: 0,
                expansion: TraversalSpec::default(),
            },
            ..ContextOptions::default()
        })
        .await
        .expect_err("invalid seed budget");
    assert_eq!(error.code(), EngineError::INVALID_ARGUMENT);
}

#[test]
fn shared_query_modes_preserve_wire_shape_and_reject_invalid_values() {
    let mut values = modes()
        .iter()
        .map(|m| serde_json::to_value(m).expect("wire"))
        .collect::<Vec<_>>();
    values.push(json!({"mode":"off"}));
    for value in values {
        let engine: GraphQueryMode = serde_json::from_value(value.clone()).expect("public mode");
        let shared: &zg_knowledge::query::GraphQueryMode = &engine;
        shared.validate().expect("valid mode");
        assert_eq!(serde_json::to_value(shared).expect("unchanged wire"), value);
    }
    for value in [
        json!({"mode":"augment","seeds":0}),
        json!({"mode":"augment","seeds":6}),
        json!({"mode":"augment","expansion":{"min_hops":0}}),
        json!({"mode":"augment","expansion":{"max_hops":9}}),
        json!({"mode":"constrain","scope":{"anchor":{"by":"position","source_id":"../a","byte_offset":0}}}),
    ] {
        let mode: GraphQueryMode = serde_json::from_value(value).expect("structured invalid mode");
        assert_eq!(
            mode.validate().expect_err("invalid limits or source").kind,
            zg_knowledge::query::QueryErrorKind::InvalidInput
        );
    }
}
