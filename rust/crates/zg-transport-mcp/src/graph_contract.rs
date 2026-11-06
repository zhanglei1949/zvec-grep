//! MCP guidance and capability projection, without changing engine request contracts.
use rmcp::model::Tool;
use serde_json::{Value, json};
use std::sync::Arc;
use zg_engine::api::{context::graph::CODE_RELATION_KINDS, explore::ExecutionOptions};

pub(super) const SEARCH_GUIDANCE: &str = " Choose graph.mode explicitly: off (default) retrieves content; augment retrieves content then adds bounded graph evidence; constrain resolves a strict graph scope before retrieval. For known callers/callees, paths or provenance, use zvec_grep_explore directly. For calls, incoming means callers and outgoing means callees. Preserve user scope; never retry failed or empty constrain as off. Read completion and truncation; Complete describes execution, not answer completeness.";

pub(super) const LEGACY_SEARCH_PURPOSE: &str = "Search a workspace index for semantic, relational, cross-file, or multi-hop evidence such as architecture, call chains, dependencies, lifecycle, data or control flow, design rationale, and comparisons.";
pub(super) const GRAPH_SEARCH_PURPOSE: &str = "Retrieve workspace content for implementation, architecture, lifecycle, design rationale, and cross-file synthesis.";

pub(super) const GRAPH_RULES: &str = concat!(
    "\n- The Agent chooses tools and parameters from the question and available evidence; the server validates and executes them, without inferring a graph plan from query text. Preserve explicit user modes and scopes.\n",
    "- Default to ordinary zvec_grep_search (omit graph or mode=off) for content. Use augment when content plus a bounded, predefined relationship supplement is useful; choose edge types, direction and hops explicitly.\n",
    "- Use constrain when retrieval must stay inside a graph-derived scope. scope.anchor is a traversal seed, not an exact scope: default traversal is outgoing calls, 1..2 hops, include_start=false. For only callers, explicitly use incoming calls, 1 hop. An empty scope stays empty; failed scope must not widen to off or unrestricted lexical search.\n",
    "- Use zvec_grep_explore directly for known-object callers/callees, paths or provenance, or after search to fill a relationship evidence gap. If an object is unknown or ambiguous, locate it or clarify first. No search-first requirement for known relationship questions.\n",
    "- incoming calls finds callers; outgoing calls finds callees. include_start includes the anchor in returned expansion/scope. boundary limits every path source; result_filter limits returned sources only. Code source paths are workspace-relative; root is absolute; semantic queries use source IDs. Omitted/null sources are unrestricted; [] is explicitly empty.\n",
    "- Read returned view, completion, truncation and degradation. Complete means the requested operation finished, not that the question is fully answered. Continue only for a material evidence gap. Tool/schema availability describes compiled capabilities, not per-workspace graph readiness.\n",
);

pub(super) fn instructions(base: &str, graph_enabled: bool) -> String {
    if graph_enabled {
        // Ordinary semantic fallback must not override a user-specified strict scope.
        base.replace(
            "- If semantic retrieval remains irrelevant, fall back to",
            "- For ordinary retrieval with no strict graph scope, if semantic retrieval remains irrelevant, fall back to",
        ) + GRAPH_RULES
    } else {
        base.to_owned()
    }
}

pub(super) fn configure_tool(tool: &mut Tool, graph_enabled: bool) {
    if !matches!(tool.name.as_ref(), "zvec_grep_search" | "zvec_grep_explore")
        || (!graph_enabled && tool.name == "zvec_grep_explore")
    {
        return;
    }
    let mut schema = Value::Object((*tool.input_schema).clone());
    if graph_enabled {
        if tool.name == "zvec_grep_search" {
            tool.description = tool.description.as_ref().map(|s| {
                format!(
                    "{}{SEARCH_GUIDANCE}",
                    s.replace(LEGACY_SEARCH_PURPOSE, GRAPH_SEARCH_PURPOSE)
                )
                .into()
            });
        }
        let relations = serde_json::to_value(CODE_RELATION_KINDS).expect("relation kinds");
        if let Some(definition) = schema.pointer_mut("/$defs/RelationKind") {
            definition["enum"] = relations;
        }
        if let Some(domain) = schema.pointer_mut("/$defs/QueryDomain") {
            domain["enum"] = json!(["code", "semantic"]);
        }
        bind_explore_domains(&mut schema);
        describe_locations(&mut schema);
        apply_allowance(&mut schema, ExecutionOptions::default());
        let examples = examples();
        schema["examples"] = Value::Array(
            examples
                .as_array()
                .expect("examples")
                .iter()
                .filter(|example| example["tool"] == tool.name.as_ref())
                .map(|example| example["arguments"].clone())
                .collect(),
        );
    } else {
        if let Some(definition) = schema.pointer_mut("/$defs/GraphQueryMode") {
            definition["oneOf"] = json!([{"type":"object", "properties":{"mode":{"const":"off","type":"string"}}, "required":["mode"], "additionalProperties":false}]);
        }
        schema["properties"]
            .as_object_mut()
            .expect("properties")
            .remove("graphExecution");
        schema["properties"]["graph"]["description"] = json!(
            "Only mode=off (ordinary retrieval) is available in this build; omission defaults to off."
        );
    }
    prune_definitions(&mut schema);
    tool.input_schema = Arc::new(schema.as_object().expect("input schema").clone());
}

fn describe_locations(schema: &mut Value) {
    let Some(locators) = schema
        .pointer_mut("/$defs/Locator/oneOf")
        .and_then(Value::as_array_mut)
    else {
        return;
    };
    for locator in locators {
        let properties = locator["properties"]
            .as_object_mut()
            .expect("locator properties");
        for (field, description) in [
            (
                "node",
                "Reuse a node reference from the same published graph view.",
            ),
            (
                "source_id",
                "Workspace-relative source path, for example src/auth.rs; disambiguates a symbol name.",
            ),
            (
                "byte_offset",
                "UTF-8 byte offset in that indexed source version.",
            ),
            (
                "name",
                "Known indexed symbol name; use locate if the name is ambiguous.",
            ),
        ] {
            if let Some(property) = properties.get_mut(field) {
                property["description"] = json!(description);
            }
        }
    }
}

fn bind_explore_domains(schema: &mut Value) {
    let Some(operations) = schema
        .pointer("/$defs/ExploreOperation/oneOf")
        .and_then(Value::as_array)
    else {
        return;
    };
    let semantic = operations
        .iter()
        .filter(|operation| {
            matches!(
                operation["properties"]["operation"]["const"].as_str(),
                Some("semantic")
            )
        })
        .cloned()
        .collect::<Vec<_>>();
    let code = operations
        .iter()
        .filter(|operation| operation["properties"]["operation"]["const"] != "semantic")
        .cloned()
        .collect::<Vec<_>>();
    schema["$defs"]["ExploreRequest"]["oneOf"] = json!([
        {"properties":{"domain":{"const":"code"},"operation":{"oneOf":code}}},
        {"properties":{"domain":{"const":"semantic"},"operation":{"oneOf":semantic}}}
    ]);
}

fn apply_allowance(schema: &mut Value, allowance: ExecutionOptions) {
    let allowance = serde_json::to_value(allowance).expect("execution allowance");
    for field in ["request_timeout_ms", "graph_timeout_ms"] {
        if let Some(property) =
            schema.pointer_mut(&format!("/$defs/ExecutionOptions/properties/{field}"))
        {
            property["maximum"] = allowance[field].clone();
        }
    }
    for (field, ceiling) in allowance["budget"].as_object().expect("budget allowance") {
        if let Some(property) =
            schema.pointer_mut(&format!("/$defs/BudgetLimits/properties/{field}"))
        {
            property["maximum"] = ceiling.clone();
        }
    }
}

// Do not leave unsupported, unreachable enum branches in tools/list for an Agent to infer from.
fn prune_definitions(schema: &mut Value) {
    fn references(value: &Value, found: &mut Vec<String>) {
        match value {
            Value::Object(fields) => {
                if let Some(name) = fields
                    .get("$ref")
                    .and_then(Value::as_str)
                    .and_then(|s| s.strip_prefix("#/$defs/"))
                {
                    found.push(name.to_owned());
                }
                for (key, value) in fields {
                    if key != "$defs" {
                        references(value, found);
                    }
                }
            }
            Value::Array(values) => {
                for value in values {
                    references(value, found);
                }
            }
            _ => {}
        }
    }
    let mut pending = Vec::new();
    references(schema, &mut pending);
    let mut reachable = std::collections::BTreeSet::new();
    while let Some(name) = pending.pop() {
        if reachable.insert(name.clone())
            && let Some(definition) = schema.pointer("/$defs").and_then(|defs| defs.get(&name))
        {
            references(definition, &mut pending);
        }
    }
    if let Some(definitions) = schema.get_mut("$defs").and_then(Value::as_object_mut) {
        definitions.retain(|name, _| reachable.contains(name));
    }
}

fn examples() -> Value {
    let mut examples: Value = serde_json::from_str(include_str!(
        "../../../compat/mcp/graph-query-examples.json"
    ))
    .expect("graph examples");
    if cfg!(windows) {
        for example in examples.as_array_mut().expect("examples") {
            example["arguments"]["root"] = json!("C:/absolute/workspace");
        }
    }
    examples
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ExploreInput, SearchInput, ZvecGrepMcpServer};
    use zg_engine::api::{
        context::graph::{Direction, GraphQueryMode},
        explore::{ExploreOperation, ExploreOptions},
    };

    fn root() -> Value {
        json!(std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("workspace"))
    }

    fn schema(name: &str, enabled: bool) -> Value {
        let router = ZvecGrepMcpServer::tool_router();
        let mut tool = router.map.get(name).expect("tool").attr.clone();
        configure_tool(&mut tool, enabled);
        Value::Object((*tool.input_schema).clone())
    }

    #[test]
    fn complete_examples_map_to_the_four_explicit_operations() {
        let examples = examples();
        let examples = examples.as_array().expect("cases");
        assert_eq!(examples.len(), 4);
        for (index, case) in examples.iter().enumerate() {
            if index == 3 {
                let input: ExploreInput =
                    serde_json::from_value(case["arguments"].clone()).expect("explore input");
                input.request.validate().expect("known relationship query");
                assert!(matches!(
                    input.request.operation,
                    ExploreOperation::Neighbors {
                        direction: Direction::Incoming,
                        ..
                    }
                ));
            } else {
                let input: SearchInput =
                    serde_json::from_value(case["arguments"].clone()).expect("search input");
                let options = input.into_request().expect("valid request");
                match (&options.graph, index) {
                    (GraphQueryMode::Off {}, 0) => {}
                    (GraphQueryMode::Augment { expansion, .. }, 1) => {
                        assert_eq!(expansion.direction, Direction::Incoming);
                        assert_eq!((expansion.min_hops, expansion.max_hops), (1, 1));
                    }
                    (GraphQueryMode::Constrain { scope }, 2) => {
                        assert_eq!(scope.traversal.direction, Direction::Incoming);
                        assert_eq!((scope.traversal.min_hops, scope.traversal.max_hops), (1, 1));
                        assert!(!scope.traversal.include_start);
                    }
                    _ => panic!("incorrect example mode"),
                }
            }
            let published = schema(case["tool"].as_str().expect("tool"), true);
            assert!(
                published["examples"]
                    .as_array()
                    .expect("schema examples")
                    .contains(&case["arguments"])
            );
        }
    }

    #[test]
    fn capability_projection_hides_unsupported_modes_domains_and_relations() {
        let unavailable = schema("zvec_grep_search", false);
        let modes = &unavailable["$defs"]["GraphQueryMode"]["oneOf"];
        assert_eq!(modes.as_array().expect("off only").len(), 1);
        assert_eq!(modes[0]["properties"]["mode"]["const"], "off");
        assert!(unavailable["properties"].get("graphExecution").is_none());
        assert!(unavailable["$defs"].get("TraversalSpec").is_none());
        assert!(unavailable.get("examples").is_none());
        let search = schema("zvec_grep_search", true);
        assert_eq!(
            search["$defs"]["RelationKind"]["enum"],
            serde_json::to_value(CODE_RELATION_KINDS).expect("relations")
        );
        let available = schema("zvec_grep_explore", true);
        assert_eq!(
            available["$defs"]["RelationKind"]["enum"],
            serde_json::to_value(CODE_RELATION_KINDS).expect("relations")
        );
        let explore = schema("zvec_grep_explore", true);
        assert_eq!(
            explore["$defs"]["QueryDomain"]["enum"],
            json!(["code", "semantic"])
        );
        let domains = &explore["$defs"]["ExploreRequest"]["oneOf"];
        let code = &domains[0]["properties"];
        let semantic = &domains[1]["properties"];
        assert_eq!(code["domain"]["const"], "code");
        assert_eq!(semantic["domain"]["const"], "semantic");
        let supported = semantic["operation"]["oneOf"]
            .as_array()
            .expect("semantic operations")
            .iter()
            .map(|operation| {
                operation["properties"]["operation"]["const"]
                    .as_str()
                    .expect("operation")
            })
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(supported, std::collections::BTreeSet::from(["semantic"]));
        for operation in code["operation"]["oneOf"]
            .as_array()
            .expect("code operations")
        {
            assert_ne!(operation["properties"]["operation"]["const"], "semantic");
        }
    }

    #[test]
    fn routing_guidance_preserves_agent_choice_strict_scope_and_execution_status() {
        let plain = instructions(crate::AGENT_INSTRUCTIONS, false);
        assert!(!plain.contains("zvec_grep_explore"));
        for base in [crate::AGENT_INSTRUCTIONS, crate::FULL_INSTRUCTIONS] {
            let graph = instructions(base, true);
            for rule in [
                "The Agent chooses tools and parameters",
                "omit graph or mode=off",
                "bounded, predefined relationship supplement",
                "failed scope must not widen to off",
                "No search-first requirement for known relationship questions",
                "Complete means the requested operation finished",
                "Preserve explicit user modes and scopes",
                "For ordinary retrieval with no strict graph scope",
            ] {
                assert!(graph.contains(rule), "missing rule: {rule}");
            }
            assert!(!graph.contains("answer requires architecture, lifecycle, call relationships"));
        }
    }

    #[test]
    fn advertised_limits_match_runtime_and_do_not_cap_general_traversal_at_two() {
        let schema = schema("zvec_grep_search", true);
        let modes = schema["$defs"]["GraphQueryMode"]["oneOf"]
            .as_array()
            .expect("modes");
        let augment = modes
            .iter()
            .find(|m| m["properties"]["mode"]["const"] == "augment")
            .expect("augment");
        let seeds = &augment["properties"]["seeds"];
        assert_eq!(
            (seeds["minimum"].clone(), seeds["maximum"].clone()),
            (json!(1), json!(5))
        );
        let hops = &augment["properties"]["expansion"]["properties"]["max_hops"];
        assert_eq!(
            (hops["minimum"].clone(), hops["maximum"].clone()),
            (json!(1), json!(2))
        );
        assert_eq!(
            augment["properties"]["expansion"]["properties"]["min_hops"]["maximum"],
            2
        );
        let traversal = &schema["$defs"]["TraversalSpec"]["properties"];
        assert_eq!(traversal["max_hops"]["maximum"], 8);
        assert_eq!(traversal["relations"]["minItems"], 1);
        assert_eq!(traversal["relations"]["uniqueItems"], true);
        let constrain = modes
            .iter()
            .find(|m| m["properties"]["mode"]["const"] == "constrain")
            .expect("constrain");
        assert!(
            constrain["required"]
                .as_array()
                .expect("required scope")
                .contains(&json!("scope"))
        );
        assert!(
            schema["$defs"]["ScopeSpec"]["required"]
                .as_array()
                .expect("required anchor")
                .contains(&json!("anchor"))
        );
        for graph in [
            json!({"mode":"augment","seeds":0}),
            json!({"mode":"augment","seeds":6}),
            json!({"mode":"augment","expansion":{"max_hops":3}}),
            json!({"mode":"augment","expansion":{"min_hops":3}}),
            json!({"mode":"augment","expansion":{"relations":[]}}),
            json!({"mode":"augment","expansion":{"min_hops":2,"max_hops":1}}),
            json!({"mode":"constrain"}),
            json!({"mode":"constrain","scope":{}}),
        ] {
            let request = serde_json::from_value::<SearchInput>(
                json!({"root":root(),"query":"401","graph":graph}),
            );
            assert!(
                request
                    .map_err(|e| e.to_string())
                    .and_then(SearchInput::into_request)
                    .is_err()
            );
        }
    }

    #[test]
    fn service_ceilings_and_positive_limits_are_advertised_for_both_tools() {
        let allowance = serde_json::to_value(ExecutionOptions::default()).expect("allowance");
        for tool in ["zvec_grep_search", "zvec_grep_explore"] {
            let schema = schema(tool, true);
            for field in ["request_timeout_ms", "graph_timeout_ms"] {
                let property = &schema["$defs"]["ExecutionOptions"]["properties"][field];
                assert_eq!(property["minimum"], 1);
                assert_eq!(property["maximum"], allowance[field]);
            }
            for (field, ceiling) in allowance["budget"].as_object().expect("budget") {
                let property = &schema["$defs"]["BudgetLimits"]["properties"][field];
                assert_eq!(property["minimum"], 1);
                assert_eq!(&property["maximum"], ceiling);
                for invalid in [0, ceiling.as_u64().expect("limit") + 1] {
                    let mut execution = allowance.clone();
                    execution["budget"][field] = json!(invalid);
                    let input: SearchInput = serde_json::from_value(json!({"root":root(), "query":"401", "graph":{"mode":"augment"}, "graphExecution":execution})).expect("typed execution");
                    assert!(input.into_request().is_err());
                }
            }
        }
        for field in ["request_timeout_ms", "graph_timeout_ms"] {
            for invalid in [0, allowance[field].as_u64().expect("limit") + 1] {
                let mut execution = allowance.clone();
                execution[field] = json!(invalid);
                let mut input: ExploreInput =
                    serde_json::from_value(examples()[3]["arguments"].clone()).expect("explore");
                input.execution = Some(serde_json::from_value(execution).expect("execution"));
                let mut options = ExploreOptions::new(input.request);
                options.execution = input.execution.expect("execution");
                assert!(options.validate().is_err());
            }
        }
    }

    #[test]
    fn anchor_defaults_and_empty_sources_remain_explicit() {
        let mode: GraphQueryMode = serde_json::from_value(
            json!({"mode":"constrain","scope":{"anchor":{"by":"name","name":"f"}}}),
        )
        .expect("scope");
        let GraphQueryMode::Constrain { scope } = mode else {
            panic!("constrain")
        };
        assert_eq!(scope.traversal.direction, Direction::Outgoing);
        assert_eq!(scope.traversal.max_hops, 2);
        assert!(!scope.traversal.include_start);
        for sources in [Value::Null, json!([]), json!(["src/auth.rs"])] {
            let filter: zg_engine::api::explore::SourceFilter =
                serde_json::from_value(json!({"sources":sources})).expect("source filter");
            filter.validate().expect("valid filter");
            assert_eq!(filter.sources.is_none(), sources.is_null());
        }
    }

    #[tokio::test]
    async fn protocol_tools_list_publishes_the_projected_catalog() {
        use rmcp::ServiceExt;
        let server = ZvecGrepMcpServer::agent_direct(Arc::new(zg_engine::ZvecGrep::new()));
        let expected = server.listed_tools();
        let (client_io, server_io) = tokio::io::duplex(64 * 1024);
        let server = tokio::spawn(async move { server.serve(server_io).await.expect("server") });
        let client = ().serve(client_io).await.expect("client");
        let tools = client.list_all_tools().await.expect("tools/list");
        assert_eq!(tools, expected);
        let search = tools
            .iter()
            .find(|tool| tool.name == "zvec_grep_search")
            .expect("search");
        let schema = Value::Object((*search.input_schema).clone());
        assert_eq!(
            schema["$defs"]["GraphQueryMode"]["oneOf"]
                .as_array()
                .expect("modes")
                .len(),
            if zg_engine::ZvecGrep::supports_explore() {
                3
            } else {
                1
            }
        );
        client.cancel().await.expect("close client");
        server
            .await
            .expect("server task")
            .cancel()
            .await
            .expect("close server");
    }
}
