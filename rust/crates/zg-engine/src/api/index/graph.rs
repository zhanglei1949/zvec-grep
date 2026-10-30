//! Model-independent graph policy and result types. No native graph dependency.
use crate::{EngineError, EngineResult};
use serde::{Deserialize, Serialize};
use std::path::{Component, Path, PathBuf};

#[derive(
    Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize, schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum GraphScope {
    #[default]
    Off,
    Code,
    Documents,
    Both,
}
impl GraphScope {
    pub(crate) fn enabled(self) -> bool {
        self != Self::Off
    }
    #[cfg(feature = "knowledge-graph-graphdb")]
    pub(crate) fn code(self) -> bool {
        matches!(self, Self::Code | Self::Both)
    }
    #[cfg(feature = "knowledge-graph-graphdb")]
    pub(crate) fn documents(self) -> bool {
        matches!(self, Self::Documents | Self::Both)
    }
}
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum GraphLanguage {
    Rust,
    Python,
}
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PackageLayout {
    Flat,
    Src,
    ModRs,
}
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GraphModuleRoot {
    pub root: PathBuf,
    pub language: GraphLanguage,
    pub namespace: String,
    pub module_prefix: Vec<String>,
    pub layout: PackageLayout,
}
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GraphRules {
    pub version: String,
}
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GraphOptions {
    pub scope: GraphScope,
    #[serde(default)]
    pub module_roots: Vec<GraphModuleRoot>,
    #[serde(default)]
    pub rules: Option<GraphRules>,
}
impl GraphOptions {
    pub(crate) fn normalized(mut self, workspace: &Path) -> EngineResult<Self> {
        let invalid = || EngineError::invalid_argument("invalid graph module configuration");
        if self
            .rules
            .as_ref()
            .is_some_and(|r| r.version != "zg-relations-v1")
        {
            return Err(EngineError::unsupported("unsupported graph rules version"));
        }
        let mut roots = std::collections::BTreeSet::new();
        let mut namespaces = std::collections::BTreeSet::new();
        let workspace = workspace.canonicalize().map_err(|_| invalid())?;
        for r in &self.module_roots {
            if r.root.is_absolute()
                || r.root.to_str().is_none()
                || r.root.to_string_lossy().contains(['\\', ':', '\0'])
                || r.root
                    .components()
                    .any(|c| !matches!(c, Component::Normal(_)))
                || r.namespace.trim().is_empty()
                || !roots.insert(r.root.clone())
                || !namespaces.insert(r.namespace.clone())
                || r.module_prefix.iter().any(String::is_empty)
                || matches!(
                    (r.language, r.layout),
                    (GraphLanguage::Rust, PackageLayout::Src)
                        | (GraphLanguage::Python, PackageLayout::ModRs)
                )
            {
                return Err(invalid());
            }
            let resolved = workspace
                .join(&r.root)
                .canonicalize()
                .map_err(|_| invalid())?;
            if !resolved.is_dir() || !resolved.starts_with(&workspace) {
                return Err(invalid());
            }
        }
        self.module_roots
            .sort_by(|a, b| (&a.root, &a.namespace).cmp(&(&b.root, &b.namespace)));
        Ok(self)
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case", tag = "state", content = "reason")]
pub enum GraphReady {
    Ready,
    NotReady(String),
    Unsupported,
    Stale,
}
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct GraphDomainResult {
    pub ready: GraphReady,
    pub node_count: u64,
    pub reference_count: u64,
    pub coverage: u64,
    /// None means an older result did not record its denominator.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_coverage: Option<u64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sources: Vec<GraphSourceCoverage>,
}
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct GraphSourceCoverage {
    pub file_id: u32,
    /// Absent for native paths that cannot be represented as a graph source.
    pub source: Option<String>,
    pub state: GraphSourceState,
    pub diagnostics: Vec<String>,
}
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum GraphSourceState {
    Ready,
    UnsupportedPath,
    NoModuleRoot,
    ModuleLanguageMismatch,
    NotIndexed,
    FactsMissing,
    IncompleteFacts,
    ResolutionPending,
}
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, schemars::JsonSchema)]
pub struct GraphIndexResult {
    pub text_published: bool,
    pub graph_candidate_committed: bool,
    pub graph_selected: bool,
    pub graph_build_id: Option<String>,
    pub mode: String,
    pub domains: std::collections::BTreeMap<String, GraphDomainResult>,
    pub diagnostics: Vec<String>,
}
impl GraphIndexResult {
    pub(crate) fn unavailable(reason: &str, text_published: bool) -> Self {
        Self {
            text_published,
            graph_candidate_committed: false,
            graph_selected: false,
            graph_build_id: None,
            mode: "unavailable".into(),
            domains: ["code", "documents"]
                .into_iter()
                .map(|domain| {
                    (
                        domain.into(),
                        GraphDomainResult {
                            ready: if matches!(
                                reason,
                                "graph_disabled" | "graph_backend_unavailable"
                            ) {
                                GraphReady::Unsupported
                            } else {
                                GraphReady::NotReady(reason.into())
                            },
                            node_count: 0,
                            reference_count: 0,
                            coverage: 0,
                            expected_coverage: None,
                            sources: Vec::new(),
                        },
                    )
                })
                .collect(),
            diagnostics: vec![reason.into()],
        }
    }
}

/// Explicit candidate reconciliation; ordinary open/index never guesses an
/// interrupted effect. This API does not load models or publish text generations.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GraphRecoveryAction {
    #[default]
    Inspect,
    Reconcile,
    Select,
    /// Resume an interrupted pre-commit import in place: reconstruct the fixed
    /// input catalog from the frozen sources, continue the persisted batch
    /// prefix and commit. Requires the identity returned by Inspect; refuses a
    /// committed, unknown or source-drifted candidate instead of replaying it.
    Resume,
    /// Verify on writable reopen, close native owners, then delete an
    /// unselected candidate under the existing workspace write admission.
    Discard,
}
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct GraphRecoveryOptions {
    pub root: Option<PathBuf>,
    /// Omission uses the selected text generation. Explicit UUID permits
    /// inspecting an unselected full-build generation without publishing it.
    pub generation: Option<String>,
    pub graph_build_id: String,
    pub action: GraphRecoveryAction,
    /// Mutating actions require the identity returned by Inspect.
    pub expected_operation_id: Option<String>,
    pub expected_attempt: Option<u64>,
    pub lock_timeout_ms: Option<u64>,
    #[serde(skip)]
    pub signal: Option<tokio_util::sync::CancellationToken>,
}
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
#[expect(
    clippy::struct_excessive_bools,
    reason = "fact completion, native commit, host selection and current eligibility are orthogonal effects"
)]
pub struct GraphRecoveryResult {
    pub graph_build_id: String,
    pub generation: String,
    pub operation_id: String,
    pub attempt: u64,
    pub facts_ready: bool,
    pub candidate_committed: bool,
    pub selected: bool,
    pub eligible: bool,
    #[serde(default)]
    pub discarded: bool,
    pub completed_batches: Option<u64>,
    pub total_batches: Option<u64>,
    pub diagnostics: Vec<String>,
}

/// Explicit recovery of a dirty text fence. Does not replay text writes or
/// restore graph readiness; a new graph build must follow successful repair.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TextStateRecoveryOptions {
    pub root: Option<PathBuf>,
    pub generation: String,
    /// False inspects only. True requires the full identity from inspection.
    pub repair: bool,
    pub expected_operation_id: Option<String>,
    pub expected_attempt: Option<u64>,
    pub expected_text_revision: Option<u64>,
    pub lock_timeout_ms: Option<u64>,
    #[serde(skip)]
    pub signal: Option<tokio_util::sync::CancellationToken>,
}
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TextStateRecoveryResult {
    pub generation: String,
    pub operation_id: String,
    pub attempt: u64,
    pub text_revision: u64,
    pub catalog_digest: String,
    pub dirty: bool,
    pub sources_match: bool,
    pub repaired: bool,
}
