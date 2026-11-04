//! Current semantic objects and necessary business history in a leased view.
use super::{Direction, EvidenceRead, QueryError, QueryResult, SourceFilter, ViewStamp, nonempty};
use crate::semantic::{
    Episode, Fact, Mention, ObjectId, SemanticChange, SemanticEntity, SourceEvidence, SourceRole,
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ObjectRef {
    pub graph_build_id: String,
    pub object: ObjectId,
}
impl ObjectRef {
    pub fn validate_for(&self, view: &ViewStamp) -> QueryResult<()> {
        self.object
            .validate()
            .map_err(|_| QueryError::invalid("invalid object identity"))?;
        if view.require_graph()?.build_id != self.graph_build_id {
            return Err(QueryError::new(
                super::QueryErrorKind::ViewMismatch,
                "view",
                "object belongs to another graph",
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SemanticQuery {
    pub space_id: String,
    pub anchor: Option<ObjectRef>,
    pub context: Option<String>,
    pub name: Option<String>,
    pub kind: Option<String>,
    pub predicate: Option<String>,
    #[serde(default)]
    pub history: bool,
    #[serde(default)]
    pub direction: Direction,
}
impl SemanticQuery {
    pub fn validate(&self) -> QueryResult<()> {
        for value in std::iter::once(&self.space_id)
            .chain(self.context.iter())
            .chain(self.name.iter())
            .chain(self.kind.iter())
            .chain(self.predicate.iter())
        {
            nonempty(value)?;
        }
        if let Some(anchor) = &self.anchor {
            nonempty(&anchor.graph_build_id)?;
            anchor
                .object
                .validate()
                .map_err(|_| QueryError::invalid("invalid semantic anchor"))?;
            if let ObjectId::SemanticEntity { identity }
            | ObjectId::Episode { identity }
            | ObjectId::Fact { identity } = &anchor.object
                && identity.space_id != self.space_id
            {
                return Err(QueryError::invalid("anchor outside requested space"));
            }
        }
        Ok(())
    }
}

/// Provider-owned authorization. Client filters only narrow this grant.
#[derive(Clone, Debug)]
pub struct SemanticAccess {
    pub spaces: Option<Vec<String>>,
    pub contexts: Option<Vec<String>>,
    pub sources: SourceFilter,
}
impl SemanticAccess {
    pub fn validate(&self) -> QueryResult<()> {
        for value in self
            .spaces
            .iter()
            .flatten()
            .chain(self.contexts.iter().flatten())
        {
            nonempty(value)?;
        }
        validate_sources(&self.sources)
    }
}
pub(super) fn validate_sources(filter: &SourceFilter) -> QueryResult<()> {
    if let Some(sources) = &filter.sources {
        if sources.len() > 2000 {
            return Err(QueryError::invalid("too many semantic sources"));
        }
        for source in sources {
            nonempty(source)?;
        }
    }
    Ok(())
}

#[derive(Clone, Copy, Debug)]
pub enum SemanticPartition {
    Sources,
    Entities,
    Episodes,
    ScalarFacts,
    Relations,
    Associations,
    Changes,
    Mentions,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, schemars::JsonSchema)]
#[serde(
    tag = "type",
    content = "value",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum SemanticValue {
    SourceNode(super::EntityRecord),
    Entity(SemanticEntity),
    Episode(Episode),
    Fact(Fact),
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, schemars::JsonSchema)]
pub struct SemanticProvenance {
    pub evidence: SourceEvidence,
    pub role: SourceRole,
    pub read: Option<EvidenceRead>,
    pub unavailable_reason: Option<String>,
}
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, schemars::JsonSchema)]
pub struct SemanticObject {
    pub reference: ObjectRef,
    pub record: SemanticValue,
    pub provenance: Vec<SemanticProvenance>,
}
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize, schemars::JsonSchema)]
pub struct SemanticReadResult {
    pub sources: Vec<crate::semantic::SemanticSource>,
    pub objects: Vec<SemanticObject>,
    pub mentions: Vec<Mention>,
    pub changes: Vec<SemanticChange>,
    pub evidence: Vec<EvidenceRead>,
}
