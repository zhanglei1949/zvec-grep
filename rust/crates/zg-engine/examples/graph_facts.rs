//! Prints G1 facts for the fixed code/document corpus. It never builds an index.
use std::path::PathBuf;
use tokio_util::sync::CancellationToken;
use zg_engine::graph_facts::{FactInput, inspect_sources};
use zg_knowledge::processing::ProcessingContext;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../benchmarks/zg-graph");
    let manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(root.join("sources.json"))?)?;
    let mut inputs = Vec::new();
    for entry in manifest["sources"]
        .as_array()
        .ok_or("invalid source manifest")?
    {
        let path = entry["path"].as_str().ok_or("missing path")?;
        let bytes = std::fs::read(root.join("corpus").join(path))?;
        if Some(zg_storage::sha256_hex(&bytes).as_str()) != entry["sha256"].as_str() {
            return Err("fixture content digest mismatch".into());
        }
        inputs.push(FactInput {
            path: path.into(),
            bytes,
            language: serde_json::from_value(entry["language"].clone())?,
            module: if entry["module"].is_null() {
                None
            } else {
                Some(serde_json::from_value(entry["module"].clone())?)
            },
        });
    }
    let context = ProcessingContext {
        max_input_bytes: 1 << 20,
        max_output_items: 10_000,
        deadline: None,
        cancellation: CancellationToken::new(),
    };
    let facts = inspect_sources("fixture-v1", &inputs, &context)?;
    println!(
        "{}",
        serde_json::to_string_pretty(
            &serde_json::json!({"stage":"G1 candidate facts only","published":false,"search_ready":false,"inspection":facts})
        )?
    );
    Ok(())
}
