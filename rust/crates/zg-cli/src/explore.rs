//! Structured exploration inputs. Parsing never opens an index or model.
use super::{CliError, CliPlan, ClientMode, resolve_from};
use clap::{Args, Subcommand};
use std::{
    io::Read,
    path::{Path, PathBuf},
};
use zg_engine::api::explore::{
    Direction, ExecutionOptions, ExploreOperation, ExploreOptions, ExploreRequest, Locator,
    NodeRef, QueryDomain, ReferenceSelector, RelationKind, SourceFilter, TraversalSpec,
};
#[derive(Debug, Args)]
pub struct ExploreArgs {
    #[arg(long, global = true)]
    pub root: Option<PathBuf>,
    #[arg(long, global = true, env = "ZVEC_GREP_MODE", default_value = "auto")]
    pub mode: ClientMode,
    #[arg(long, global = true, env = "ZVEC_GREP_HOME")]
    pub home: Option<PathBuf>,
    /// Complete `ExploreRequest` JSON, mutually exclusive with subcommands.
    #[arg(long)]
    pub request: Option<PathBuf>,
    #[arg(long, global = true, default_value_t = 30_000)]
    pub timeout_ms: u64,
    #[arg(long, global = true, default_value_t = 5_000)]
    pub graph_timeout_ms: u64,
    #[arg(long, global = true)]
    pub lock_timeout_ms: Option<u64>,
    #[command(subcommand)]
    pub command: Option<ExploreCommand>,
}
#[derive(Debug, Args)]
pub struct LocatorArgs {
    #[arg(long)]
    pub name: Option<String>,
    #[arg(long)]
    pub source: Option<String>,
    #[arg(long)]
    pub byte_offset: Option<u64>,
    #[arg(long)]
    pub node_id: Option<String>,
    #[arg(long)]
    pub graph_build_id: Option<String>,
    #[arg(long)]
    pub kind: Option<String>,
}
#[derive(Debug, Args)]
pub struct NeighborArgs {
    #[command(flatten)]
    pub anchor: LocatorArgs,
    #[arg(long, value_delimiter = ',', default_value = "calls")]
    pub relations: Vec<String>,
    #[arg(long, default_value = "outgoing")]
    pub direction: String,
}
#[derive(Debug, Args)]
pub struct TraverseArgs {
    #[command(flatten)]
    pub neighbor: NeighborArgs,
    #[arg(long, default_value_t = 1)]
    pub min_hops: u32,
    #[arg(long, default_value_t = 2)]
    pub max_hops: u32,
    #[arg(long)]
    pub include_start: bool,
}
#[derive(Debug, Subcommand)]
pub enum ExploreCommand {
    Locate(LocatorArgs),
    Callers(LocatorArgs),
    Callees(LocatorArgs),
    Neighbors(NeighborArgs),
    Expand(TraverseArgs),
    Path {
        #[command(flatten)]
        start: TraverseArgs,
        #[arg(long)]
        to_name: String,
        #[arg(long)]
        to_source: Option<String>,
    },
    References {
        #[arg(long)]
        owner: Option<String>,
        #[arg(long)]
        source: Option<String>,
    },
}
fn bad(message: impl Into<String>) -> CliError {
    CliError::InvalidExplore(message.into())
}
fn value<T: serde::de::DeserializeOwned>(text: &str) -> Result<T, CliError> {
    serde_json::from_value(serde_json::Value::String(text.into()))
        .map_err(|_| bad(format!("invalid explore value: {text}")))
}
impl LocatorArgs {
    fn locator(self) -> Result<Locator, CliError> {
        let kind = self.kind.as_deref().map(value).transpose()?;
        match (
            self.name,
            self.source,
            self.byte_offset,
            self.node_id,
            self.graph_build_id,
        ) {
            (Some(name), source_id, None, None, None) => Ok(Locator::Name {
                name,
                source_id,
                kind,
            }),
            (None, Some(source_id), Some(byte_offset), None, None) => Ok(Locator::Position {
                source_id,
                byte_offset,
                kind,
            }),
            (None, None, None, Some(node_id), Some(graph_build_id)) if kind.is_none() => {
                Ok(Locator::Node {
                    node: NodeRef {
                        node_id,
                        graph_build_id,
                    },
                })
            }
            _ => Err(bad(
                "use --name [--source], --source with --byte-offset, or --node-id with --graph-build-id",
            )),
        }
    }
}
impl TraverseArgs {
    fn split(self) -> Result<(Locator, TraversalSpec), CliError> {
        Ok((
            self.neighbor.anchor.locator()?,
            TraversalSpec {
                relations: self
                    .neighbor
                    .relations
                    .iter()
                    .map(|s| value(s))
                    .collect::<Result<_, _>>()?,
                direction: value(&self.neighbor.direction)?,
                min_hops: self.min_hops,
                max_hops: self.max_hops,
                include_start: self.include_start,
                boundary: SourceFilter::default(),
                result_filter: SourceFilter::default(),
            },
        ))
    }
}
pub(crate) fn json_file<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T, CliError> {
    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .map_err(|e| bad(format!("read {}: {e}", path.display())))?
        .take(1_048_577)
        .read_to_end(&mut bytes)
        .map_err(|e| bad(e.to_string()))?;
    if bytes.len() > 1_048_576 {
        return Err(bad("JSON input exceeds 1 MiB"));
    }
    serde_json::from_slice(&bytes).map_err(|e| bad(format!("invalid {}: {e}", path.display())))
}
pub(crate) fn plan(args: ExploreArgs, current: &Path) -> Result<CliPlan, CliError> {
    let request = match (args.request, args.command) {
        (Some(file), None) => json_file(&resolve_from(current, Some(&file)))?,
        (None, Some(command)) => {
            let operation = match command {
                ExploreCommand::Locate(a) => ExploreOperation::Locate {
                    target: a.locator()?,
                },
                ExploreCommand::Callers(a) => ExploreOperation::Neighbors {
                    anchor: a.locator()?,
                    relations: vec![RelationKind::Calls],
                    direction: Direction::Incoming,
                },
                ExploreCommand::Callees(a) => ExploreOperation::Neighbors {
                    anchor: a.locator()?,
                    relations: vec![RelationKind::Calls],
                    direction: Direction::Outgoing,
                },
                ExploreCommand::Neighbors(a) => ExploreOperation::Neighbors {
                    anchor: a.anchor.locator()?,
                    relations: a
                        .relations
                        .iter()
                        .map(|s| value(s))
                        .collect::<Result<_, _>>()?,
                    direction: value(&a.direction)?,
                },
                ExploreCommand::Expand(a) => {
                    let (anchor, traversal) = a.split()?;
                    ExploreOperation::Expand { anchor, traversal }
                }
                ExploreCommand::Path {
                    start,
                    to_name,
                    to_source,
                } => {
                    let (start, traversal) = start.split()?;
                    ExploreOperation::ShortestPath {
                        start,
                        target: Locator::Name {
                            name: to_name,
                            source_id: to_source,
                            kind: None,
                        },
                        traversal,
                    }
                }
                ExploreCommand::References { owner, source } => ExploreOperation::References {
                    selector: match (owner, source) {
                        (Some(name), source_id) => ReferenceSelector::Owner {
                            owner: Locator::Name {
                                name,
                                source_id,
                                kind: None,
                            },
                        },
                        (None, Some(source_id)) => ReferenceSelector::Source { source_id },
                        _ => return Err(bad("references requires --owner or --source")),
                    },
                },
            };
            ExploreRequest {
                domain: QueryDomain::Code,
                operation,
                boundary: SourceFilter::default(),
                result_filter: SourceFilter::default(),
            }
        }
        _ => return Err(bad("choose an explore subcommand or --request JSON file")),
    };
    let options = ExploreOptions {
        root: Some(resolve_from(current, args.root.as_deref())),
        request,
        execution: ExecutionOptions {
            request_timeout_ms: args.timeout_ms,
            graph_timeout_ms: args.graph_timeout_ms,
            ..Default::default()
        },
        lock_timeout_ms: args.lock_timeout_ms,
        signal: None,
    };
    options.validate().map_err(|e| bad(e.to_string()))?;
    Ok(CliPlan::Explore {
        mode: args.mode,
        home: args.home,
        request: Box::new(options),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn callers_and_json_requests_use_the_same_typed_operation() {
        let cli = super::super::Cli::try_parse_from([
            "zg",
            "explore",
            "--mode",
            "direct",
            "--root",
            "/tmp/project",
            "callers",
            "--name",
            "target",
        ])
        .expect("CLI");
        let CliPlan::Explore { request, .. } = cli.into_plan(PathBuf::from("/tmp")).expect("plan")
        else {
            panic!("explore");
        };
        assert!(matches!(
            request.request.operation,
            ExploreOperation::Neighbors {
                direction: Direction::Incoming,
                ..
            }
        ));
        assert_eq!(request.root, Some(PathBuf::from("/tmp/project")));
    }
    #[test]
    fn exploration_has_no_model_or_refresh_flags() {
        for flag in ["--refresh", "--embedding", "--auto-update"] {
            assert!(
                super::super::Cli::try_parse_from([
                    "zg", "explore", flag, "wait", "locate", "--name", "x"
                ])
                .is_err()
            );
        }
        assert!(
            super::super::Cli::try_parse_from([
                "zg",
                "explore",
                "locate",
                "--name",
                "x",
                "--byte-offset",
                "1",
                "--source",
                "a.rs"
            ])
            .expect("parse")
            .into_plan(PathBuf::from("/tmp"))
            .is_err()
        );
        assert!(
            super::super::Cli::try_parse_from([
                "zg",
                "explore",
                "neighbors",
                "--name",
                "x",
                "--max-hops",
                "3"
            ])
            .is_err(),
            "neighbors must not silently ignore hop parameters"
        );
    }
}
