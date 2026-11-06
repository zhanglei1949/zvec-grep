use serde::{Deserialize, Serialize};
fn default_page_size() -> u16 {
    super::DEFAULT_PAGE_SIZE
}

use super::{NodeRef, QueryError, QueryResult, nonempty, source_id};

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum QueryDomain {
    Code,
    Documents,
    Semantic,
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
    Mentions,
    SemanticRelation,
    EpisodeLink,
}

/// Edge direction: incoming calls finds callers; outgoing calls finds callees; both follows either.
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

pub use crate::relations::EntityKind;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, schemars::JsonSchema)]
#[serde(tag = "by", rename_all = "snake_case", deny_unknown_fields)]
pub enum Locator {
    Object {
        object: super::ObjectRef,
    },
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
            Self::Object { object } => object
                .object
                .validate()
                .map_err(|_| QueryError::invalid("invalid object locator")),
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

/// Source selection: code uses workspace-relative paths; semantic uses source IDs.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize, schemars::JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct SourceFilter {
    /// Omitted/null means unrestricted; [] is explicitly empty, not unrestricted.
    #[schemars(length(max = 2000))]
    pub sources: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kinds: Option<Vec<String>>,
}
impl SourceFilter {
    pub fn validate(&self) -> QueryResult<()> {
        if let Some(kinds) = &self.kinds {
            if kinds.len() > 256 {
                return Err(QueryError::invalid("too many node kinds"));
            }
            for kind in kinds {
                nonempty(kind)?;
            }
        }
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

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, schemars::JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct TraversalSpec {
    /// Edge types to follow; choose at least one, without duplicates.
    #[schemars(length(min = 1, max = 9), extend("uniqueItems" = true))]
    pub relations: Vec<RelationKind>,
    /// incoming finds callers; outgoing finds callees for calls edges.
    pub direction: Direction,
    /// Minimum path length. Must satisfy 1 <= `min_hops` <= `max_hops`.
    #[schemars(range(min = 1, max = 8))]
    pub min_hops: u32,
    /// Maximum path length (up to 8; Augment allows at most 2).
    #[schemars(range(min = 1, max = 8))]
    pub max_hops: u32,
    /// Include the anchor in expansion results. Paths include their endpoints.
    pub include_start: bool,
    /// Restrict every source admitted along the traversal path.
    pub boundary: SourceFilter,
    /// Restrict returned sources; traversal may pass through other sources.
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
        if self.relations.is_empty() || self.relations.len() > 9 {
            return Err(QueryError::invalid("expected one to nine relation kinds"));
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

/// Text to search: full includes nested entities; own includes only the selected entity's own text.
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
    /// Traversal seed, not an exact scope by itself. Disambiguate names with `source_id`.
    pub anchor: Locator,
    /// Select entities whose text may be searched. Default: outgoing calls, 1..2 hops, excludes anchor.
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

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, schemars::JsonSchema)]
#[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
pub enum GraphQueryMode {
    /// Ordinary retrieval (default). No graph expansion or graph-derived scope.
    Off {},
    /// Retrieve normally, then add bounded graph evidence from the top retrieved entities.
    Augment {
        /// Number of retrieved entities used as graph seeds (default 5, range 1..5).
        #[serde(default = "default_seeds")]
        #[schemars(range(min = 1, max = 5))]
        seeds: u32,
        /// Explicit edge types, direction and hops; at most 2 hops. Ordinary ranking stays unchanged.
        #[serde(default)]
        #[schemars(schema_with = "augmentation_traversal_schema")]
        expansion: TraversalSpec,
    },
    /// Resolve a strict graph-derived scope first, then retrieve only within its mapped text.
    Constrain {
        /// Required anchor and traversal. Empty/failed scope must not widen to ordinary retrieval.
        scope: ScopeSpec,
    },
}

fn augmentation_traversal_schema(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
    let mut schema = <TraversalSpec as schemars::JsonSchema>::json_schema(generator);
    for field in ["min_hops", "max_hops"] {
        *schema
            .pointer_mut(&format!("/properties/{field}/maximum"))
            .expect("traversal hop ceiling") = serde_json::json!(2);
    }
    schema
}
impl GraphQueryMode {
    #[must_use]
    pub const fn is_off(&self) -> bool {
        matches!(self, Self::Off {})
    }

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

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, schemars::JsonSchema)]
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

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, schemars::JsonSchema)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub enum ExploreOperation {
    RelationTypes {
        anchor: Locator,
        direction: Direction,
    },
    /// Read semantic objects, facts, provenance or correction history (domain=semantic).
    Semantic { query: super::SemanticQuery },
    /// Resolve an object by name, source position or a previously returned node reference.
    Locate { target: Locator },
    /// Read one-hop relationships for a known object; incoming calls returns its callers.
    Neighbors {
        anchor: Locator,
        #[schemars(length(min = 1, max = 9), extend("uniqueItems" = true))]
        relations: Vec<RelationKind>,
        direction: Direction,
    },
    /// Traverse from a known object with explicit direction, edge types and hop bounds.
    Expand {
        anchor: Locator,
        traversal: TraversalSpec,
    },
    /// Find a bounded shortest path between two known objects.
    ShortestPath {
        start: Locator,
        target: Locator,
        traversal: TraversalSpec,
    },
    /// Read resolved or unresolved source references and their evidence.
    References { selector: ReferenceSelector },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ExploreRequest {
    #[serde(default = "default_page_size")]
    #[schemars(range(min = 1, max = 256))]
    pub page_size: u16,
    #[serde(default)]
    pub cursor: Option<String>,
    #[serde(default)]
    pub relation_filters: Vec<super::RelationSelector>,
    /// Domain selects the available relation and locator types.
    pub domain: QueryDomain,
    pub operation: ExploreOperation,
    /// Restrict every source along the traversal path.
    #[serde(default)]
    pub boundary: SourceFilter,
    /// Restrict returned sources only, without restricting intermediate path sources.
    #[serde(default)]
    pub result_filter: SourceFilter,
}
impl ExploreRequest {
    pub fn validate(&self) -> QueryResult<()> {
        if matches!(self.operation, ExploreOperation::Semantic { .. })
            && self.domain != QueryDomain::Semantic
        {
            return Err(QueryError::invalid(
                "semantic operation requires semantic domain",
            ));
        }
        if self.page_size == 0 || self.page_size > super::MAX_PAGE_SIZE {
            return Err(QueryError::invalid("page_size must be in 1..=256"));
        }
        if !self.relation_filters.is_empty()
            && !matches!(
                self.operation,
                ExploreOperation::Neighbors { .. } | ExploreOperation::RelationTypes { .. }
            )
        {
            return Err(QueryError::invalid(
                "relation_filters are supported by neighbors and relation_types",
            ));
        }
        for (index, selector) in self.relation_filters.iter().enumerate() {
            selector.validate()?;
            if self.relation_filters[..index]
                .iter()
                .any(|previous| previous.edge_type == selector.edge_type)
            {
                return Err(QueryError::invalid("duplicate relation filter"));
            }
            if let ExploreOperation::Neighbors { relations, .. } = &self.operation
                && !relations.contains(&selector.edge_type)
            {
                return Err(QueryError::invalid(
                    "relation filter must narrow the selected edge types",
                ));
            }
        }
        if self.cursor.is_some()
            && !matches!(
                self.operation,
                ExploreOperation::RelationTypes { .. } | ExploreOperation::Neighbors { .. }
            )
        {
            return Err(QueryError::invalid("operation does not support pagination"));
        }
        if self.domain == QueryDomain::Semantic {
            if matches!(self.operation, ExploreOperation::Semantic { .. })
                && (self.boundary.kinds.is_some() || self.result_filter.kinds.is_some())
            {
                return Err(QueryError::invalid(
                    "semantic selection uses query.kind; kinds filters apply to graph steps and results",
                ));
            }
            super::semantic::validate_sources(&self.boundary)?;
            super::semantic::validate_sources(&self.result_filter)?;
        } else {
            self.boundary.validate()?;
            self.result_filter.validate()?;
        }
        match &self.operation {
            ExploreOperation::RelationTypes { anchor, .. } => anchor.validate(),
            ExploreOperation::Semantic { query } => query.validate(),
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
        if let Self::Object { object } = self {
            object.validate_for(view)?;
        }
        Ok(())
    }
}
impl ExploreRequest {
    pub fn validate_for(&self, view: &super::ViewStamp) -> QueryResult<()> {
        self.validate()?;
        view.require_graph()?;
        match &self.operation {
            ExploreOperation::Semantic { query } => {
                if let Some(anchor) = &query.anchor {
                    anchor.validate_for(view)?;
                }
                Ok(())
            }
            ExploreOperation::Locate { target } => target.validate_for(view),
            ExploreOperation::Neighbors { anchor, .. }
            | ExploreOperation::RelationTypes { anchor, .. }
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
