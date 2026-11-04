use super::SemanticEntity;
use crate::{
    Result,
    processing::{canonical_json, identifier},
};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

#[derive(
    Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize, schemars::JsonSchema,
)]
#[serde(deny_unknown_fields)]
pub struct Identity {
    pub space_id: String,
    pub id: String,
}
impl Identity {
    pub fn validate(&self) -> Result<()> {
        identifier(&self.space_id)?;
        identifier(&self.id)
    }
    /// JSON tuple encoding keeps delimiters and Unicode unambiguous.
    pub fn storage_key(&self) -> Result<String> {
        canonical_json(&(&self.space_id, &self.id))
    }
}

#[derive(
    Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize, schemars::JsonSchema,
)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ObjectId {
    SourceNode { node_id: String },
    SemanticEntity { identity: Identity },
    Episode { identity: Identity },
    Fact { identity: Identity },
}
impl ObjectId {
    pub fn validate(&self) -> Result<()> {
        match self {
            Self::SourceNode { node_id } => identifier(node_id),
            Self::SemanticEntity { identity }
            | Self::Episode { identity }
            | Self::Fact { identity } => identity.validate(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EntityInput {
    pub space_id: String,
    pub id: Option<String>,
    pub kind: String,
    pub name: String,
    #[serde(default)]
    pub aliases: Vec<String>,
    // Omission preserves attributes; an explicitly supplied value must be an object.
    #[serde(
        default,
        deserialize_with = "provided_attributes",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "Map<String, Value>")]
    pub attributes: Option<Map<String, Value>>,
}
fn provided_attributes<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<Option<Map<String, Value>>, D::Error> {
    Map::deserialize(deserializer).map(Some)
}
impl EntityInput {
    pub fn validate(&self) -> Result<()> {
        identifier(&self.space_id)?;
        if let Some(id) = &self.id {
            identifier(id)?;
        }
        identifier(&self.kind)?;
        identifier(&self.name)?;
        for alias in &self.aliases {
            identifier(alias)?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum IdentityResolution {
    New,
    Unique(Identity),
    Ambiguous(Vec<Identity>),
    Conflict(Identity),
}

/// Existing records are a validated catalog. Matching is exact, case-sensitive
/// UTF-8 within one space; names and aliases are not global identity keys.
pub fn resolve_identity(
    input: &EntityInput,
    existing: &[SemanticEntity],
) -> Result<IdentityResolution> {
    input.validate()?;
    let mut candidates = Vec::new();
    for entity in existing
        .iter()
        .filter(|e| e.identity.space_id == input.space_id)
    {
        if let Some(id) = &input.id {
            if entity.identity.id == *id {
                return Ok(
                    if entity.kind != input.kind
                        || input
                            .attributes
                            .as_ref()
                            .is_some_and(|a| a != &entity.attributes)
                    {
                        IdentityResolution::Conflict(entity.identity.clone())
                    } else {
                        IdentityResolution::Unique(entity.identity.clone())
                    },
                );
            }
        } else if entity.kind == input.kind
            && std::iter::once(&input.name)
                .chain(&input.aliases)
                .any(|name| name == &entity.name || entity.aliases.contains(name))
        {
            candidates.push(entity);
        }
    }
    candidates.sort_by(|a, b| a.identity.cmp(&b.identity));
    Ok(match candidates.as_slice() {
        [] => IdentityResolution::New,
        [entity]
            if input
                .attributes
                .as_ref()
                .is_some_and(|a| a != &entity.attributes) =>
        {
            IdentityResolution::Conflict(entity.identity.clone())
        }
        [entity] => IdentityResolution::Unique(entity.identity.clone()),
        _ => IdentityResolution::Ambiguous(candidates.iter().map(|e| e.identity.clone()).collect()),
    })
}
