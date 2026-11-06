//! Stable, view-bound bookmarks; bookmarks never grant access.
use super::{
    Direction, QueryError, QueryResult, RelationKind, RelationRecord, SourceFilter, ViewStamp,
    nonempty,
};
use serde::{Deserialize, Serialize};

pub const DEFAULT_PAGE_SIZE: u16 = 64;
pub const MAX_PAGE_SIZE: u16 = 256;
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RelationSelector {
    pub edge_type: RelationKind,
    #[serde(default)]
    pub relation_types: Option<Vec<String>>,
}
impl RelationSelector {
    pub fn validate(&self) -> QueryResult<()> {
        if self.relation_types.is_some()
            && !matches!(
                self.edge_type,
                RelationKind::SemanticRelation | RelationKind::EpisodeLink
            )
        {
            return Err(QueryError::invalid(
                "structural edges do not have business relation types",
            ));
        }
        if let Some(types) = &self.relation_types {
            if types.len() > 256 {
                return Err(QueryError::invalid("too many relation types"));
            }
            for value in types {
                nonempty(value)?;
                if self.edge_type == RelationKind::EpisodeLink
                    && !matches!(value.as_str(), "REPLY_TO" | "RESULT_OF")
                {
                    return Err(QueryError::invalid("unknown episode relation"));
                }
            }
        }
        Ok(())
    }
}
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Ord, PartialOrd, Serialize)]
pub struct EdgeKey {
    pub relation_type: String,
    pub source: String,
    pub target: String,
    pub id: String,
}
impl RelationRecord {
    #[must_use]
    pub fn key(&self) -> EdgeKey {
        EdgeKey {
            relation_type: self.relation_type.clone().unwrap_or_default(),
            source: self.source.clone(),
            target: self.target.clone(),
            id: self.id.clone(),
        }
    }
}
pub struct NeighborRead<'a> {
    pub anchor: &'a crate::semantic::ObjectId,
    pub selector: &'a RelationSelector,
    pub direction: Direction,
    pub after: Option<&'a EdgeKey>,
    pub skip_group: bool,
    pub boundary: &'a SourceFilter,
    pub contexts: Option<&'a [String]>,
    pub spaces: Option<&'a [String]>,
}
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, schemars::JsonSchema)]
pub struct AvailableRelationType {
    pub edge_type: RelationKind,
    pub relation_type: Option<String>,
    pub direction: Direction,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct Cursor {
    pub view: ViewStamp,
    pub query: String,
    pub authorization: String,
    pub partition: usize,
    pub after: Option<EdgeKey>,
    pub skip_group: bool,
}
impl Cursor {
    pub fn decode(
        value: &str,
        view: &ViewStamp,
        query: &str,
        authorization: &str,
    ) -> QueryResult<Self> {
        if value.len() > 32768 {
            return Err(QueryError::invalid("cursor is too large"));
        }
        let cursor: Self =
            serde_json::from_str(value).map_err(|_| QueryError::invalid("invalid cursor"))?;
        view.require_same(&cursor.view)?;
        if cursor.query != query || cursor.authorization != authorization {
            return Err(QueryError::invalid(
                "cursor conditions or authorization changed",
            ));
        }
        Ok(cursor)
    }
    pub fn encode(&self) -> QueryResult<String> {
        let encoded = serde_json::to_string(self)
            .map_err(|_| QueryError::invalid("cursor serialization failed"))?;
        if encoded.len() > 32768 {
            return Err(QueryError::new(
                super::QueryErrorKind::ResourceExhausted,
                "cursor",
                "bookmark exceeds the supported size",
            ));
        }
        Ok(encoded)
    }
}
