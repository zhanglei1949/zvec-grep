//! Explicit wire-to-KG conversion. No engine types flow back into KG.
use super::{
    BodyRegion, Direction, EntityKind, GraphQueryMode, Locator, RelationKind, ScopeSpec,
    SourceFilter, TraversalSpec,
};
use zg_knowledge::query as kg;

impl From<RelationKind> for kg::RelationKind {
    fn from(value: RelationKind) -> Self {
        match value {
            RelationKind::Contains => Self::Contains,
            RelationKind::Calls => Self::Calls,
            RelationKind::Imports => Self::Imports,
            RelationKind::Links => Self::Links,
            RelationKind::Extends => Self::Extends,
            RelationKind::Implements => Self::Implements,
        }
    }
}
impl From<EntityKind> for kg::EntityKind {
    fn from(value: EntityKind) -> Self {
        match value {
            EntityKind::File => Self::File,
            EntityKind::Module => Self::Module,
            EntityKind::Class => Self::Class,
            EntityKind::Interface => Self::Interface,
            EntityKind::Function => Self::Function,
            EntityKind::Method => Self::Method,
            EntityKind::Field => Self::Field,
            EntityKind::Section => Self::Section,
            EntityKind::Type => Self::Type,
            EntityKind::Constant => Self::Constant,
        }
    }
}
impl From<Direction> for kg::Direction {
    fn from(value: Direction) -> Self {
        match value {
            Direction::Incoming => Self::Incoming,
            Direction::Outgoing => Self::Outgoing,
            Direction::Both => Self::Both,
        }
    }
}
impl From<Locator> for kg::Locator {
    fn from(value: Locator) -> Self {
        match value {
            Locator::Node { node } => Self::Node {
                node: kg::NodeRef {
                    graph_build_id: node.graph_build_id,
                    node_id: node.node_id,
                },
            },
            Locator::Position {
                source_id,
                byte_offset,
                kind,
            } => Self::Position {
                source_id,
                byte_offset,
                kind: kind.map(Into::into),
            },
            Locator::Name {
                name,
                source_id,
                kind,
            } => Self::Name {
                name,
                source_id,
                kind: kind.map(Into::into),
            },
        }
    }
}
impl From<SourceFilter> for kg::SourceFilter {
    fn from(value: SourceFilter) -> Self {
        Self {
            sources: value.sources,
        }
    }
}
impl From<TraversalSpec> for kg::TraversalSpec {
    fn from(value: TraversalSpec) -> Self {
        Self {
            relations: value.relations.into_iter().map(Into::into).collect(),
            direction: value.direction.into(),
            min_hops: value.min_hops,
            max_hops: value.max_hops,
            include_start: value.include_start,
            boundary: value.boundary.into(),
            result_filter: value.result_filter.into(),
        }
    }
}
impl From<ScopeSpec> for kg::ScopeSpec {
    fn from(value: ScopeSpec) -> Self {
        Self {
            anchor: value.anchor.into(),
            traversal: value.traversal.into(),
            region: match value.region {
                BodyRegion::Full => kg::BodyRegion::Full,
                BodyRegion::Own => kg::BodyRegion::Own,
            },
        }
    }
}
impl TryFrom<GraphQueryMode> for kg::GraphQueryMode {
    type Error = crate::EngineError;
    fn try_from(value: GraphQueryMode) -> Result<Self, Self::Error> {
        value.validate()?;
        Ok(match value {
            GraphQueryMode::Off {} => Self::Off {},
            GraphQueryMode::Augment { seeds, expansion } => Self::Augment {
                seeds,
                expansion: expansion.into(),
            },
            GraphQueryMode::Constrain { scope } => Self::Constrain {
                scope: scope.into(),
            },
        })
    }
}
