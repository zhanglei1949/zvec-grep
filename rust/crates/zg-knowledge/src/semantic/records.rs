use super::{Identity, ObjectId, same_space};
use crate::{Error, Result, processing::identifier, query::EvidenceRef};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Number, Value};

#[derive(
    Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize, schemars::JsonSchema,
)]
#[serde(deny_unknown_fields)]
pub struct SourceRef {
    pub source_id: String,
    pub source_version: String,
}
impl SourceRef {
    pub fn validate(&self) -> Result<()> {
        identifier(&self.source_id)?;
        identifier(&self.source_version)
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SourceEvidence {
    pub source: SourceRef,
    pub locator: Option<EvidenceRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub episode: Option<Identity>,
}
impl SourceEvidence {
    pub fn validate(&self) -> Result<()> {
        self.source.validate()?;
        if let Some(episode) = &self.episode {
            episode.validate()?;
        }
        if let Some(locator) = &self.locator {
            if locator.source_id != self.source.source_id {
                return Err(Error::InvalidInput("semantic evidence source mismatch"));
            }
            locator
                .validate_position()
                .map_err(|_| Error::InvalidInput("invalid semantic evidence position"))?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SemanticEntity {
    pub identity: Identity,
    pub kind: String,
    pub name: String,
    pub aliases: Vec<String>,
    pub attributes: Map<String, Value>,
}
impl SemanticEntity {
    pub fn validate(&self) -> Result<()> {
        self.identity.validate()?;
        identifier(&self.kind)?;
        identifier(&self.name)?;
        for alias in &self.aliases {
            identifier(alias)?;
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum EpisodeState {
    Active,
    Withdrawn,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Episode {
    pub identity: Identity,
    pub kind: String,
    pub source: SourceRef,
    pub context: String,
    pub occurred_at_ms: Option<u64>,
    pub ingested_at_ms: u64,
    pub content_ref: String,
    pub state: EpisodeState,
}
impl Episode {
    pub fn validate(&self) -> Result<()> {
        self.identity.validate()?;
        identifier(&self.kind)?;
        self.source.validate()?;
        identifier(&self.context)?;
        identifier(&self.content_ref)
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum FactState {
    Active,
    Superseded,
    Retracted,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, schemars::JsonSchema)]
#[serde(untagged)]
pub enum ScalarValue {
    String(String),
    Number(Number),
    Boolean(bool),
    Null,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, schemars::JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum FactObject {
    Entity { identity: Identity },
    Scalar { value: ScalarValue },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Fact {
    pub identity: Identity,
    pub subject: Identity,
    #[serde(alias = "predicate")]
    pub relation_type: String,
    pub object: FactObject,
    pub context: String,
    pub state: FactState,
    pub supersedes: Option<Identity>,
    /// KG acceptance/change time. Ordinary inputs do not control the stored value.
    pub changed_at_ms: u64,
    /// First acceptance time. NULL is reserved for facts migrated without a known time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_at_ms: Option<u64>,
    pub attributes: Map<String, Value>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum EpisodeRelation {
    ReplyTo,
    ResultOf,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EpisodeLink {
    pub identity: Identity,
    pub source: Identity,
    pub target: Identity,
    pub relation_type: EpisodeRelation,
}
impl EpisodeLink {
    pub fn validate(&self) -> Result<()> {
        self.identity.validate()?;
        self.source.validate()?;
        self.target.validate()?;
        same_space(&self.identity.space_id, &self.source.space_id)?;
        same_space(&self.identity.space_id, &self.target.space_id)?;
        if self.source == self.target {
            return Err(Error::InvalidInput("episode link cannot self-reference"));
        }
        Ok(())
    }
}
impl Fact {
    pub fn validate(&self) -> Result<()> {
        self.identity.validate()?;
        self.subject.validate()?;
        same_space(&self.subject.space_id, &self.identity.space_id)?;
        identifier(&self.relation_type)?;
        identifier(&self.context)?;
        if let FactObject::Entity { identity } = &self.object {
            identity.validate()?;
            same_space(&identity.space_id, &self.identity.space_id)?;
        }
        if let Some(previous) = &self.supersedes {
            previous.validate()?;
            same_space(&previous.space_id, &self.identity.space_id)?;
            if previous == &self.identity {
                return Err(Error::InvalidInput("fact cannot supersede itself"));
            }
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SourceRole {
    Support,
    Correction,
    /// Last contribution removed by a complete replacement or source deletion.
    Withdrawn,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SourceAssociation {
    pub target: ObjectId,
    pub evidence: SourceEvidence,
    pub role: SourceRole,
}
impl SourceAssociation {
    pub fn validate(&self) -> Result<()> {
        self.target.validate()?;
        if !matches!(
            self.target,
            ObjectId::SemanticEntity { .. } | ObjectId::Fact { .. }
        ) {
            return Err(Error::InvalidInput(
                "source association requires an entity or fact",
            ));
        }
        self.evidence.validate()
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Mention {
    pub ref_id: String,
    pub owner_id: String,
    pub target: Identity,
    pub evidence: EvidenceRef,
}
impl Mention {
    pub fn validate(&self) -> Result<()> {
        identifier(&self.ref_id)?;
        identifier(&self.owner_id)?;
        self.target.validate()?;
        identifier(&self.evidence.source_id)?;
        self.evidence
            .validate_position()
            .map_err(|_| Error::InvalidInput("invalid mention position"))?;
        if self.evidence.reference_id.as_deref() != Some(self.ref_id.as_str()) {
            return Err(Error::InvalidInput("mention reference ID mismatch"));
        }
        Ok(())
    }
}
