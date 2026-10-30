use serde::{Deserialize, Serialize};

use super::{NodeRef, QueryError, QueryResult, nonempty, source_id};

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum QueryDomain {
    Code,
    Documents,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RelationKind {
    Contains,
    Calls,
    Imports,
    Links,
    Extends,
    Implements,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    Incoming,
    #[default]
    Outgoing,
    Both,
}

pub use crate::relations::EntityKind;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
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
    pub fn validate(&self) -> QueryResult<()> {
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
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct SourceFilter {
    /// None means unrestricted; Some([]) is explicitly empty.
    pub sources: Option<Vec<String>>,
}
impl SourceFilter {
    pub fn validate(&self) -> QueryResult<()> {
        if let Some(ids) = &self.sources {
            if ids.len() > 2000 {
                return Err(QueryError::invalid("too many source filters"));
            }
            for id in ids {
                source_id(id)?;
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
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
    pub fn validate(&self) -> QueryResult<()> {
        if self.min_hops == 0 || self.min_hops > self.max_hops || self.max_hops > 8 {
            return Err(QueryError::invalid(
                "hops must satisfy 1 <= min <= max <= 8",
            ));
        }
        if self.relations.is_empty() || self.relations.len() > 6 {
            return Err(QueryError::invalid(
                "expected one to six business relation kinds",
            ));
        }
        for (i, kind) in self.relations.iter().enumerate() {
            if self.relations[..i].contains(kind) {
                return Err(QueryError::invalid("duplicate relation kind"));
            }
        }
        self.boundary.validate()?;
        self.result_filter.validate()
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BodyRegion {
    #[default]
    Full,
    Own,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ScopeSpec {
    pub anchor: Locator,
    #[serde(default)]
    pub traversal: TraversalSpec,
    #[serde(default)]
    pub region: BodyRegion,
}
impl ScopeSpec {
    pub fn validate(&self) -> QueryResult<()> {
        self.anchor.validate()?;
        self.traversal.validate()
    }
}

const fn default_seeds() -> u32 {
    5
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
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
    pub fn validate(&self) -> QueryResult<()> {
        match self {
            Self::Off {} => Ok(()),
            Self::Augment { seeds, expansion } => {
                if !(1..=5).contains(seeds) {
                    return Err(QueryError::invalid(
                        "seed count must be between one and five",
                    ));
                }
                expansion.validate()?;
                if expansion.max_hops > 2 {
                    return Err(QueryError::invalid(
                        "augmentation supports at most two hops",
                    ));
                }
                Ok(())
            }
            Self::Constrain { scope } => scope.validate(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "by", rename_all = "snake_case", deny_unknown_fields)]
pub enum ReferenceSelector {
    Id {
        reference_id: String,
        view: super::ViewStamp,
    },
    Owner {
        owner: Locator,
    },
    Source {
        source_id: String,
    },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub enum ExploreOperation {
    Locate {
        target: Locator,
    },
    Neighbors {
        anchor: Locator,
        relations: Vec<RelationKind>,
        direction: Direction,
    },
    Expand {
        anchor: Locator,
        traversal: TraversalSpec,
    },
    ShortestPath {
        start: Locator,
        target: Locator,
        traversal: TraversalSpec,
    },
    References {
        selector: ReferenceSelector,
    },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ExploreRequest {
    pub domain: QueryDomain,
    pub operation: ExploreOperation,
    #[serde(default)]
    pub boundary: SourceFilter,
    #[serde(default)]
    pub result_filter: SourceFilter,
}
impl ExploreRequest {
    pub fn validate(&self) -> QueryResult<()> {
        self.boundary.validate()?;
        self.result_filter.validate()?;
        match &self.operation {
            ExploreOperation::Locate { target } => target.validate(),
            ExploreOperation::Neighbors {
                anchor,
                relations,
                direction,
            } => {
                anchor.validate()?;
                TraversalSpec {
                    relations: relations.clone(),
                    direction: *direction,
                    max_hops: 1,
                    ..TraversalSpec::default()
                }
                .validate()
            }
            ExploreOperation::Expand { anchor, traversal } => {
                anchor.validate()?;
                traversal.validate()
            }
            ExploreOperation::ShortestPath {
                start,
                target,
                traversal,
            } => {
                start.validate()?;
                target.validate()?;
                traversal.validate()
            }
            ExploreOperation::References { selector } => match selector {
                ReferenceSelector::Id { reference_id, view } => {
                    nonempty(reference_id)?;
                    view.validate()
                }
                ReferenceSelector::Owner { owner } => owner.validate(),
                ReferenceSelector::Source { source_id: id } => source_id(id),
            },
        }
    }
}

impl Locator {
    pub fn validate_for(&self, view: &super::ViewStamp) -> QueryResult<()> {
        self.validate()?;
        view.validate()?;
        if let Self::Node { node } = self {
            view.check_node(node)?;
        }
        Ok(())
    }
}
impl ExploreRequest {
    pub fn validate_for(&self, view: &super::ViewStamp) -> QueryResult<()> {
        self.validate()?;
        view.require_graph()?;
        match &self.operation {
            ExploreOperation::Locate { target } => target.validate_for(view),
            ExploreOperation::Neighbors { anchor, .. }
            | ExploreOperation::Expand { anchor, .. } => anchor.validate_for(view),
            ExploreOperation::ShortestPath { start, target, .. } => {
                start.validate_for(view)?;
                target.validate_for(view)
            }
            ExploreOperation::References { selector } => match selector {
                ReferenceSelector::Id { view: expected, .. } => view.require_same(expected),
                ReferenceSelector::Owner { owner } => owner.validate_for(view),
                ReferenceSelector::Source { .. } => Ok(()),
            },
        }
    }
}
impl GraphQueryMode {
    pub fn validate_for(&self, view: &super::ViewStamp) -> QueryResult<()> {
        self.validate()?;
        view.validate()?;
        if !matches!(self, Self::Off {}) {
            view.require_graph()?;
        }
        if let Self::Constrain { scope } = self {
            scope.anchor.validate_for(view)?;
        }
        Ok(())
    }
}

impl Default for GraphQueryMode {
    fn default() -> Self {
        Self::Off {}
    }
}
