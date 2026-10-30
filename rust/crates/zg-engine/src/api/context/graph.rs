//! Query wire types stay available without the optional KG implementation.
use serde::{Deserialize, Serialize};

#[cfg(feature = "knowledge-graph")]
mod knowledge;

fn nonempty(value: &str) -> crate::EngineResult<()> {
    if value.trim().is_empty() || value.len() > 4096 || value.contains('\0') {
        return Err(crate::EngineError::invalid_argument(
            "expected a non-empty bounded identifier",
        ));
    }
    Ok(())
}
fn source_id(value: &str) -> crate::EngineResult<()> {
    nonempty(value)?;
    if value.contains(['\\', '\0', ':']) || value.split('/').any(|p| matches!(p, "" | "." | "..")) {
        return Err(crate::EngineError::invalid_argument(
            "expected normalized workspace-relative source",
        ));
    }
    Ok(())
}
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NodeRef {
    pub graph_build_id: String,
    pub node_id: String,
}
impl NodeRef {
    /// Checks the structure of a graph query parameter without opening resources.
    /// # Errors
    /// Returns invalid argument for malformed identifiers, paths or bounds.
    pub fn validate(&self) -> crate::EngineResult<()> {
        nonempty(&self.graph_build_id)?;
        nonempty(&self.node_id)
    }
}
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum QueryDomain {
    Code,
    Documents,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RelationKind {
    Contains,
    Calls,
    Imports,
    Links,
    Extends,
    Implements,
}

#[derive(
    Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize, schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    Incoming,
    #[default]
    Outgoing,
    Both,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum EntityKind {
    File,
    Module,
    Class,
    Interface,
    Function,
    Method,
    Field,
    Section,
    Type,
    Constant,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, schemars::JsonSchema)]
#[serde(tag = "by", rename_all = "snake_case", deny_unknown_fields)]
pub enum Locator {
    Node {
        node: NodeRef,
    },
    Position {
        source_id: String,
        byte_offset: u64,
        kind: Option<EntityKind>,
    },
    Name {
        name: String,
        source_id: Option<String>,
        kind: Option<EntityKind>,
    },
}
impl Locator {
    /// Checks the structure of a graph query parameter without opening resources.
    /// # Errors
    /// Returns invalid argument for malformed identifiers, paths or bounds.
    pub fn validate(&self) -> crate::EngineResult<()> {
        match self {
            Self::Node { node } => node.validate(),
            Self::Position { source_id: id, .. } => source_id(id),
            Self::Name {
                name,
                source_id: id,
                ..
            } => {
                nonempty(name)?;
                if let Some(id) = id {
                    source_id(id)?;
                }
                Ok(())
            }
        }
    }
}

/// Path admission and result selection are intentionally separate.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize, schemars::JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct SourceFilter {
    /// None means unrestricted; Some([]) is explicitly empty.
    pub sources: Option<Vec<String>>,
}
impl SourceFilter {
    /// Checks the structure of a graph query parameter without opening resources.
    /// # Errors
    /// Returns invalid argument for malformed identifiers, paths or bounds.
    pub fn validate(&self) -> crate::EngineResult<()> {
        if let Some(ids) = &self.sources {
            if ids.len() > 2000 {
                return Err(crate::EngineError::invalid_argument(
                    "too many source filters",
                ));
            }
            for id in ids {
                source_id(id)?;
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, schemars::JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct TraversalSpec {
    pub relations: Vec<RelationKind>,
    pub direction: Direction,
    pub min_hops: u32,
    pub max_hops: u32,
    /// Include the anchor in expansion results. Paths include their endpoints.
    pub include_start: bool,
    pub boundary: SourceFilter,
    pub result_filter: SourceFilter,
}
impl Default for TraversalSpec {
    fn default() -> Self {
        Self {
            relations: vec![RelationKind::Calls],
            direction: Direction::Outgoing,
            min_hops: 1,
            max_hops: 2,
            include_start: false,
            boundary: SourceFilter::default(),
            result_filter: SourceFilter::default(),
        }
    }
}
impl TraversalSpec {
    /// Checks the structure of a graph query parameter without opening resources.
    /// # Errors
    /// Returns invalid argument for malformed identifiers, paths or bounds.
    pub fn validate(&self) -> crate::EngineResult<()> {
        if self.min_hops == 0 || self.min_hops > self.max_hops || self.max_hops > 8 {
            return Err(crate::EngineError::invalid_argument(
                "hops must satisfy 1 <= min <= max <= 8",
            ));
        }
        if self.relations.is_empty() || self.relations.len() > 6 {
            return Err(crate::EngineError::invalid_argument(
                "expected one to six business relation kinds",
            ));
        }
        for (i, kind) in self.relations.iter().enumerate() {
            if self.relations[..i].contains(kind) {
                return Err(crate::EngineError::invalid_argument(
                    "duplicate relation kind",
                ));
            }
        }
        self.boundary.validate()?;
        self.result_filter.validate()
    }
}

#[derive(
    Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize, schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum BodyRegion {
    #[default]
    Full,
    Own,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ScopeSpec {
    pub anchor: Locator,
    #[serde(default)]
    pub traversal: TraversalSpec,
    #[serde(default)]
    pub region: BodyRegion,
}
impl ScopeSpec {
    /// Checks the structure of a graph query parameter without opening resources.
    /// # Errors
    /// Returns invalid argument for malformed identifiers, paths or bounds.
    pub fn validate(&self) -> crate::EngineResult<()> {
        self.anchor.validate()?;
        self.traversal.validate()
    }
}

const fn default_seeds() -> u32 {
    5
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, schemars::JsonSchema)]
#[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
pub enum GraphQueryMode {
    Off {},
    Augment {
        #[serde(default = "default_seeds")]
        seeds: u32,
        #[serde(default)]
        expansion: TraversalSpec,
    },
    Constrain {
        scope: ScopeSpec,
    },
}
impl GraphQueryMode {
    /// Checks the structure of a graph query parameter without opening resources.
    /// # Errors
    /// Returns invalid argument for malformed identifiers, paths or bounds.
    pub fn validate(&self) -> crate::EngineResult<()> {
        match self {
            Self::Off {} => Ok(()),
            Self::Augment { seeds, expansion } => {
                if !(1..=5).contains(seeds) {
                    return Err(crate::EngineError::invalid_argument(
                        "seed count must be between one and five",
                    ));
                }
                expansion.validate()?;
                if expansion.max_hops > 2 {
                    return Err(crate::EngineError::invalid_argument(
                        "augmentation supports at most two hops",
                    ));
                }
                Ok(())
            }
            Self::Constrain { scope } => scope.validate(),
        }
    }
}

impl GraphQueryMode {
    #[must_use]
    pub const fn is_off(&self) -> bool {
        matches!(self, Self::Off {})
    }

    /// Checks compiled query capabilities without opening a workspace or model.
    /// # Errors
    /// Returns invalid argument for malformed parameters and unsupported for an
    /// unavailable backend, mode or relation.
    pub fn ensure_supported(&self, rg: bool) -> crate::EngineResult<()> {
        self.validate()?;
        if !self.is_off() {
            if rg {
                return Err(crate::EngineError::invalid_argument(
                    "rg supports only graph mode off",
                ));
            }
            if !cfg!(feature = "knowledge-graph-graphdb") {
                return Err(crate::EngineError::unsupported(
                    "graph augmentation requires the graphdb backend",
                ));
            }
            let expansion = match self {
                Self::Augment { expansion, .. } => expansion,
                Self::Constrain { scope } => &scope.traversal,
                Self::Off {} => return Ok(()),
            };
            if expansion.relations.iter().any(|r| {
                !matches!(
                    r,
                    RelationKind::Contains | RelationKind::Calls | RelationKind::Imports
                )
            }) {
                return Err(crate::EngineError::unsupported(
                    "code graph queries support CONTAINS, CALLS and IMPORTS",
                ));
            }
        }
        Ok(())
    }
}

impl Default for GraphQueryMode {
    fn default() -> Self {
        Self::Off {}
    }
}
