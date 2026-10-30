//! Typed graph options for indexed queries; parsing opens no workspace.
use super::{CliError, GraphModeArg, QueryArgs, explore, resolve_from};
use std::path::Path;
use zg_engine::api::{
    context::graph::{GraphQueryMode, TraversalSpec},
    explore::ExecutionOptions,
};

pub(super) fn options(
    args: &QueryArgs,
    current_dir: &Path,
) -> Result<(GraphQueryMode, Option<ExecutionOptions>), CliError> {
    let graph = match &args.graph_request {
        Some(path) => explore::json_file(&resolve_from(current_dir, Some(path)))?,
        None => match args.graph_mode.unwrap_or(GraphModeArg::Off) {
            GraphModeArg::Off => zg_engine::api::context::graph::GraphQueryMode::Off {},
            GraphModeArg::Augment => zg_engine::api::context::graph::GraphQueryMode::Augment {
                seeds: 5,
                expansion: TraversalSpec::default(),
            },
        },
    };
    graph
        .validate()
        .map_err(|e| CliError::InvalidExplore(e.message().into()))?;
    if args.rg && !graph.is_off() {
        return Err(CliError::InvalidExplore(
            "rg supports only graph mode off".into(),
        ));
    }
    let graph_execution = args
        .graph_execution
        .as_ref()
        .map(|p| {
            explore::json_file::<zg_engine::api::explore::ExecutionOptions>(&resolve_from(
                current_dir,
                Some(p),
            ))
        })
        .transpose()?;
    if let Some(execution) = graph_execution {
        execution
            .validate()
            .map_err(|e| CliError::InvalidExplore(e.message().into()))?;
        if graph.is_off() {
            return Err(CliError::InvalidExplore(
                "graph execution options require graph mode augment".into(),
            ));
        }
    }
    Ok((graph, graph_execution))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Cli, CliPlan};

    #[test]
    fn query_graph_flags_keep_text_routes_and_share_the_json_contract() {
        let directory = tempfile::tempdir().expect("input");
        std::fs::write(directory.path().join("graph.json"), r#"{"mode":"augment","seeds":2,"expansion":{"max_hops":1,"direction":"incoming","boundary":{"sources":[]}}}"#).expect("graph JSON");
        std::fs::write(
            directory.path().join("execution.json"),
            r#"{"request_timeout_ms":1000,"budget":{"nodes":50}}"#,
        )
        .expect("execution JSON");
        let cli = Cli::try_parse_from([
            "zg",
            "query",
            "caller",
            "--fts",
            "target",
            "--graph-request",
            "graph.json",
            "--graph-execution",
            "execution.json",
        ])
        .expect("parse");
        let CliPlan::Query { request, .. } = cli.into_plan(directory.path().into()).expect("plan")
        else {
            panic!("query");
        };
        assert_eq!(request.queries, ["caller"]);
        assert_eq!(request.routes[0].query, "target");
        assert_eq!(request.graph_execution.expect("limits").budget.nodes, 50);
        let GraphQueryMode::Augment { seeds, expansion } = request.graph else {
            panic!("augment");
        };
        assert_eq!(seeds, 2);
        assert_eq!(expansion.boundary.sources, Some(vec![]));
        assert_eq!(
            expansion.direction,
            zg_engine::api::context::graph::Direction::Incoming
        );
    }

    #[test]
    fn graph_modes_are_explicit_and_conflicting_inputs_do_not_fall_back_to_off() {
        assert!(
            Cli::try_parse_from([
                "zg",
                "query",
                "x",
                "--graph-mode",
                "augment",
                "--graph-request",
                "missing.json"
            ])
            .is_err()
        );
        assert!(
            Cli::try_parse_from(["zg", "query", "--rg", "x", "--graph-mode", "augment"])
                .expect("parse")
                .into_plan("/tmp".into())
                .is_err()
        );
        let CliPlan::Query { request, .. } =
            Cli::try_parse_from(["zg", "query", "x", "--graph-mode=augment"])
                .expect("parse")
                .into_plan("/tmp".into())
                .expect("plan")
        else {
            panic!("query");
        };
        assert!(matches!(
            request.graph,
            GraphQueryMode::Augment { seeds: 5, .. }
        ));
    }
}
