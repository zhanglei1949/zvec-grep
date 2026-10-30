//! Explicit graph candidate inspection and reconciliation example.
use zg_engine::{
    ZvecGrep,
    api::index::graph::{GraphRecoveryAction, GraphRecoveryOptions},
};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    if args.len() != 2 && args.len() != 3 && args.len() != 6 {
        return Err(
            "usage: ROOT BUILD_ID [GENERATION] [reconcile|select|discard OPERATION_ID ATTEMPT]"
                .into(),
        );
    }
    let action = match args.get(3).map(String::as_str) {
        None => GraphRecoveryAction::Inspect,
        Some("reconcile") => GraphRecoveryAction::Reconcile,
        Some("select") => GraphRecoveryAction::Select,
        Some("resume") => GraphRecoveryAction::Resume,
        Some("discard") => GraphRecoveryAction::Discard,
        _ => return Err("unknown action".into()),
    };
    let result = ZvecGrep::new()
        .recover_graph(GraphRecoveryOptions {
            root: Some(args[0].clone().into()),
            graph_build_id: args[1].clone(),
            generation: args.get(2).cloned(),
            action,
            expected_operation_id: args.get(4).cloned(),
            expected_attempt: args.get(5).map(|s| s.parse()).transpose()?,
            ..Default::default()
        })
        .await?;
    println!("{}", serde_json::to_string_pretty(&result)?);
    Ok(())
}
