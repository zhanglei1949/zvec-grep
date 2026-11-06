//! Keep the benchmark's offline fixtures tied to the real product tools/list response.
#![cfg(feature = "knowledge-graph-graphdb")]
use rmcp::ServiceExt;
use serde_json::json;
use std::{path::Path, sync::Arc};
use zg_engine::ZvecGrep;
use zg_transport_mcp::ZvecGrepMcpServer;

#[tokio::test]
async fn swe_qa_fixture_matches_product_tools_list() {
    let graphdb = ZvecGrep::supports_explore();
    let server = ZvecGrepMcpServer::agent_direct(Arc::new(ZvecGrep::new()));
    let (client_io, server_io) = tokio::io::duplex(64 * 1024);
    let server = tokio::spawn(async move { server.serve(server_io).await.expect("server") });
    let client = ().serve(client_io).await.expect("client");
    let tools = client.list_all_tools().await.expect("product tools/list");
    let search = tools
        .iter()
        .find(|tool| tool.name == "zvec_grep_search")
        .expect("search");
    let names = tools
        .iter()
        .map(|tool| tool.name.as_ref())
        .collect::<Vec<_>>();
    assert_eq!(names.contains(&"zvec_grep_explore"), graphdb);
    let mut catalog =
        json!({"graphdb_enabled": graphdb, "tool_names": names, "search_tool": search});
    // Examples use a platform-specific absolute root; normalize that placeholder only.
    if let Some(examples) = catalog
        .pointer_mut("/search_tool/inputSchema/examples")
        .and_then(serde_json::Value::as_array_mut)
    {
        for example in examples {
            example["root"] = json!("/absolute/workspace");
        }
    }
    client.cancel().await.expect("close client");
    server
        .await
        .expect("server task")
        .cancel()
        .await
        .expect("close server");

    // Both fixed-query arms require the same graphdb-enabled candidate package.
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../benchmarks/swe-qa-bench/tests/fixtures")
        .join("mcp-search-graphdb.json");
    if std::env::var("ZG_BENCH_UPDATE_MCP_FIXTURES").as_deref() == Ok("1") {
        std::fs::create_dir_all(fixture.parent().expect("fixture directory"))
            .expect("create directory");
        std::fs::write(
            &fixture,
            serde_json::to_string_pretty(&catalog).expect("catalog JSON") + "\n",
        )
        .expect("write fixture");
    }
    let expected: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(&fixture)
            .expect("capture fixture with ZG_BENCH_UPDATE_MCP_FIXTURES=1"),
    )
    .expect("fixture JSON");
    assert_eq!(
        catalog, expected,
        "product catalog changed; recapture and review the benchmark fixture"
    );
}
