//! Export native relationship tool definitions for offline parser checks.

use std::{io, sync::Arc};

use serde_json::json;
use zg_engine::ZvecGrep;
use zg_transport_mcp::ZvecGrepMcpServer;

fn main() -> serde_json::Result<()> {
    let legacy = ZvecGrepMcpServer::agent_direct(Arc::new(ZvecGrep::new()));
    let canonical = legacy.clone().with_canonical_relationship_schema();
    let tools = |server: &ZvecGrepMcpServer| {
        server
            .listed_tools()
            .into_iter()
            .filter(|tool| {
                matches!(
                    tool.name.as_ref(),
                    "zvec_grep_callers" | "zvec_grep_callees"
                )
            })
            .collect::<Vec<_>>()
    };
    serde_json::to_writer_pretty(
        io::stdout(),
        &json!({"legacy": tools(&legacy), "canonical": tools(&canonical)}),
    )
}
