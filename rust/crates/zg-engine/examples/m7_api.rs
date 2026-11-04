use serde::Deserialize;
use std::path::PathBuf;
use zg_engine::{
    ZvecGrep,
    api::{
        context::{
            ContextOptions,
            options::{ContextRoute, ContextRouteMode, RefreshPolicy},
        },
        explore::{ExploreOptions, ExploreRequest},
        index::graph::SemanticBuildOptions,
    },
};
#[derive(Deserialize)]
struct Manifest {
    #[serde(rename = "storageGeneration")]
    generation: String,
}
#[derive(Deserialize)]
struct Head {
    #[serde(rename = "graphBuildId")]
    build: String,
}
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args_os().skip(1);
    let action = args.next().ok_or("apply or explore required")?;
    let root = PathBuf::from(args.next().ok_or("root required")?).canonicalize()?;
    let path = args.next().ok_or("input JSON required")?;
    if args.next().is_some() {
        return Err("unexpected argument".into());
    }
    let engine = ZvecGrep::new();
    if action == "apply" {
        let manifest: Manifest =
            serde_json::from_slice(&std::fs::read(root.join(".zvec-grep/manifest.json"))?)?;
        let head: Head = serde_json::from_slice(&std::fs::read(
            root.join(".zvec-grep/generations")
                .join(&manifest.generation)
                .join("graph/HEAD.json"),
        )?)?;
        let input = serde_json::from_slice(&std::fs::read(path)?)?;
        let result = engine
            .apply_semantics(SemanticBuildOptions {
                root: Some(root),
                generation: manifest.generation,
                operation_id: uuid::Uuid::new_v4().to_string(),
                expected_graph_build_id: head.build,
                input,
                lock_timeout_ms: None,
                signal: None,
            })
            .await?;
        println!("{}", serde_json::to_string(&result)?);
    } else if action == "context" {
        #[derive(Deserialize)]
        struct Input {
            query: String,
            graph: zg_engine::api::context::graph::GraphQueryMode,
        }
        let input: Input = serde_json::from_slice(&std::fs::read(path)?)?;
        let options = ContextOptions {
            root: Some(root),
            routes: vec![ContextRoute {
                mode: ContextRouteMode::Fts,
                query: input.query,
            }],
            graph: input.graph,
            auto_update: false,
            refresh: Some(RefreshPolicy::Off),
            ..ContextOptions::default()
        };
        println!(
            "{}",
            serde_json::to_string(&engine.context(options).await?)?
        );
    } else if action == "explore" {
        let request: ExploreRequest = serde_json::from_slice(&std::fs::read(path)?)?;
        let mut options = ExploreOptions::new(request);
        options.root = Some(root);
        println!(
            "{}",
            serde_json::to_string(&engine.explore(options).await?)?
        );
    } else {
        return Err("unknown action".into());
    }
    assert_eq!(engine.runtime_snapshot().loaded_models, 0);
    engine.close();
    Ok(())
}
