//! Inspect first; repair only with the full identity returned by inspection.
use zg_engine::{ZvecGrep, api::index::graph::TextStateRecoveryOptions};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    if args.len() != 2 && args.len() != 6 {
        return Err("usage: ROOT GENERATION [repair OPERATION_ID ATTEMPT TEXT_REVISION]".into());
    }
    let repair = args.len() == 6;
    if repair && args[2] != "repair" {
        return Err("unknown action".into());
    }
    let result = ZvecGrep::new()
        .recover_text_state(TextStateRecoveryOptions {
            root: Some(args[0].clone().into()),
            generation: args[1].clone(),
            repair,
            expected_operation_id: args.get(3).cloned(),
            expected_attempt: args.get(4).map(|s| s.parse()).transpose()?,
            expected_text_revision: args.get(5).map(|s| s.parse()).transpose()?,
            ..Default::default()
        })
        .await?;
    println!("{}", serde_json::to_string_pretty(&result)?);
    Ok(())
}
