//! Build and maintain a real host graph in a caller-owned temporary workspace.
//! This example never enables remote embedding or adds a public CLI/MCP command.
use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Instant,
};
use zg_engine::{
    ZvecGrep,
    api::index::{
        IndexOptions,
        graph::GraphOptions,
        options::{Device, EmbeddingModelSpec, WorkspaceChange},
        progress::{IndexProgress, IndexProgressReporter},
    },
};

#[tokio::main]
#[expect(
    clippy::too_many_lines,
    reason = "keep temporary-root validation, local model setup and diagnostic output in one example"
)]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args_os().skip(1);
    let root =
        PathBuf::from(args.next().ok_or("temporary workspace root required")?).canonicalize()?;
    let mut temporary_roots = vec![std::env::temp_dir().canonicalize()?];
    if let Ok(public_temporary) = PathBuf::from("/tmp").canonicalize() {
        temporary_roots.push(public_temporary);
    }
    if !temporary_roots
        .iter()
        .any(|base| root != *base && root.starts_with(base))
    {
        return Err(
            "ROOT must be a caller-owned directory beneath the OS temporary directory".into(),
        );
    }
    let graph = match args.next().as_deref().and_then(|v| v.to_str()) {
        None => return Err("GRAPH_OPTIONS_JSON or - required".into()),
        Some("-") => None,
        Some(value) => Some(serde_json::from_str::<GraphOptions>(value)?),
    };
    let model_cache = PathBuf::from(args.next().ok_or("model cache required")?);
    let changes = args
        .next()
        .as_deref()
        .and_then(|v| v.to_str())
        .map(serde_json::from_str::<Vec<WorkspaceChange>>)
        .transpose()?
        .unwrap_or_default();
    let graph_only = match args.next().as_deref().and_then(|v| v.to_str()) {
        None => false,
        Some("--graph-only") => true,
        Some(_) => return Err("unknown graph build mode".into()),
    };
    if args.next().is_some() {
        return Err(
            "usage: ROOT GRAPH_OPTIONS_JSON|- MODEL_CACHE [WORKSPACE_CHANGES_JSON] [--graph-only]"
                .into(),
        );
    }
    let events = Arc::new(Mutex::new(Vec::<IndexProgress>::new()));
    let captured = Arc::clone(&events);
    let options = IndexOptions {
        root: Some(root),
        graph,
        changes,
        embedding: if graph_only {
            None
        } else {
            Some(EmbeddingModelSpec {
                reference: "local/potion-code-16m-v2".into(),
                revision: None,
                cache_dir: None,
                endpoint: None,
                device: Device::Cpu,
            })
        },
        device: if graph_only { None } else { Some(Device::Cpu) },
        runtime_device: if graph_only { None } else { Some(Device::Cpu) },
        model_cache: if graph_only { None } else { Some(model_cache) },
        allow_remote: false,
        on_progress: Some(IndexProgressReporter::new(move |event| {
            captured
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(event);
        })),
        ..IndexOptions::default()
    };
    let started = Instant::now();
    let result = ZvecGrep::new().index(options).await?;
    let wall_time_micros = started.elapsed().as_micros();
    let events = events
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let model_events = events.iter().filter(|e| e.embedding.is_some()).count();
    let model_downloaded_bytes = events
        .iter()
        .filter_map(|e| e.embedding.as_ref()?.downloaded_bytes)
        .max()
        .unwrap_or(0);
    if let Some(graph) = &result.graph
        && graph_only
    {
        assert_eq!(graph.mode, "graph_only");
        assert!(!graph.text_published);
        assert_eq!(model_events, 0, "graph-only must not load a model");
        assert_eq!(model_downloaded_bytes, 0);
    }
    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({
            "result": result,
            "wall_time_micros": wall_time_micros,
            "model_events": model_events,
            "model_downloaded_bytes": model_downloaded_bytes,
            "model": "local/potion-code-16m-v2",
            "device": "cpu",
            "remote_allowed": false,
        }))?
    );
    Ok(())
}
