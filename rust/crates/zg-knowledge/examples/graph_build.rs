//! Committed candidate inspection only; host publication remains with engine.
use zg_knowledge::{
    graph::{
        GraphDbStore, GraphErrorKind, GraphLimits,
        build::{BuildScope, BuildSpec},
    },
    processing::{ProcessingContext, stable_digest},
    relations::bound::FactCatalog,
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args_os().skip(1);
    let catalog_path = args.next().ok_or("catalog JSON input required")?;
    let spec_path = args.next().ok_or("BuildSpec JSON input required")?;
    let path = args.next().ok_or("new candidate directory required")?;
    if args.next().is_some() {
        return Err("usage: CATALOG_JSON BUILD_SPEC_JSON NEW_CANDIDATE_DIR".into());
    }
    let catalog: FactCatalog = serde_json::from_slice(&std::fs::read(catalog_path)?)?;
    let spec: BuildSpec = serde_json::from_slice(&std::fs::read(spec_path)?)?;
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
    let store = GraphDbStore::create_build(&path, limits.clone())?;
    let counts = store.write_facts(&catalog, &context)?;
    let mut invalid = spec.clone();
    invalid.attempt = 0;
    let error = store
        .commit_build(&invalid, &context)
        .expect_err("attempt zero must be rejected");
    assert_eq!(error.kind, GraphErrorKind::InvalidInput);
    store.ensure_no_build_commit()?;
    let confirmed = store.commit_build(&spec, &context)?;
    let replayed = store.commit_build(&spec, &context)?;
    assert_eq!(replayed, confirmed);
    store.checkpoint()?;
    drop(store);

    let store = GraphDbStore::open(&path, true, limits)?;
    let reopened = store.read_facts(&context)?;
    assert_eq!(reopened, catalog);
    let inspected = store
        .inspect_build(&spec, &context)?
        .ok_or("committed proof missing after reopen")?;
    assert_eq!(inspected, confirmed);
    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({
            "counts": counts,
            "catalog_digest": stable_digest(&catalog)?,
            "graph_build_id": confirmed.graph_build_id,
            "receipt_id": confirmed.receipt_id,
            "facts_digest": confirmed.facts_digest,
            "scope": match spec.scope {
                BuildScope::Code => "code",
                BuildScope::Documents => "documents",
                BuildScope::Both => "both",
            },
            "candidate_committed": true,
            "published": false,
            "search_ready": false
        }))?
    );
    Ok(())
}
