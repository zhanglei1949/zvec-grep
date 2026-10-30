//! Explicit candidate inspection only; never selects a host graph generation.
use zg_knowledge::{
    graph::{GraphDbStore, GraphLimits, schema},
    processing::{ProcessingContext, stable_digest},
    relations::bound::FactCatalog,
};
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args_os().skip(1);
    let input = args.next().ok_or("catalog JSON input required")?;
    if input == std::ffi::OsStr::new("--schema") {
        println!("{}", schema::statements().join(";\n"));
        return Ok(());
    }
    let path = args.next().ok_or("new candidate directory required")?;
    let catalog: FactCatalog = serde_json::from_slice(&std::fs::read(input)?)?;
    let context = ProcessingContext {
        max_input_bytes: 64 << 20,
        max_output_items: 30_000_000,
        deadline: None,
        cancellation: tokio_util::sync::CancellationToken::new(),
    };
    let limits = GraphLimits {
        max_rows: 4_000_000,
        ..GraphLimits::default()
    };
    let store = GraphDbStore::create(&path, limits.clone())?;
    let counts = store.write_facts(&catalog, &context)?;
    store.checkpoint()?;
    drop(store);
    let store = GraphDbStore::open(path, true, limits)?;
    let reopened = store.read_facts(&context)?;
    if reopened != catalog {
        return Err("candidate round-trip mismatch".into());
    }
    println!(
        "{}",
        serde_json::to_string_pretty(
            &serde_json::json!({"counts":counts,"catalog_digest":stable_digest(&reopened)?,"published":false,"search_ready":false})
        )?
    );
    Ok(())
}
