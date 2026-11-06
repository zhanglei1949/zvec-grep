use super::{
    EntityInput, Episode, Fact, IdentityResolution, Mention, ObjectId, SemanticEntity,
    SourceAssociation, SourceRef, resolve_identity,
};
use crate::{
    Error, Result,
    processing::{ProcessingContext, identifier, stable_digest},
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SemanticSourceState {
    Unspecified,
    Complete,
    Partial,
    Failed,
    Deleted,
}

/// Current source version and the last version verified by complete input or an explicit correction.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SemanticSource {
    pub source: SourceRef,
    pub state: SemanticSourceState,
    pub semantic_version: Option<String>,
}
impl SemanticSource {
    pub fn validate(&self) -> Result<()> {
        self.source.validate()?;
        if let Some(version) = &self.semantic_version {
            identifier(version)?;
        }
        if (self.state == SemanticSourceState::Complete
            && self.semantic_version.as_ref() != Some(&self.source.source_version))
            || (self.state == SemanticSourceState::Deleted && self.semantic_version.is_some())
        {
            return Err(Error::InvalidInput("inconsistent semantic source version"));
        }
        Ok(())
    }
}

/// Resolved current records, shared by the existing candidate and native codec.
/// This contains no source bodies, vectors or graph snapshots.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SemanticCatalog {
    pub sources: Vec<SemanticSource>,
    pub entities: Vec<SemanticEntity>,
    pub episodes: Vec<Episode>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub episode_links: Vec<super::EpisodeLink>,
    pub facts: Vec<Fact>,
    pub associations: Vec<SourceAssociation>,
    pub mentions: Vec<Mention>,
    /// Last complete positions whose source-node binding is no longer current.
    pub deferred_mentions: Vec<Mention>,
    pub changes: Vec<super::SemanticChange>,
    pub last_operation: Option<super::SemanticOperation>,
}
impl SemanticCatalog {
    /// Bind a new entity once, or reuse the identity already in this catalog.
    /// The returned identity is persisted with the entity, not regenerated on read.
    pub fn register_entity(
        &mut self,
        input: &EntityInput,
        source: &SourceRef,
        ordinal: usize,
    ) -> Result<IdentityResolution> {
        source.validate()?;
        let resolution = resolve_identity(input, &self.entities)?;
        if resolution != IdentityResolution::New {
            return Ok(resolution);
        }
        let identity = super::Identity {
            space_id: input.space_id.clone(),
            id: match &input.id {
                Some(id) => id.clone(),
                None => stable_digest(&("semantic-entity-v1", source, ordinal, input))?,
            },
        };
        if self
            .entities
            .iter()
            .any(|entity| entity.identity == identity)
        {
            return Err(Error::Conflict("generated semantic identity conflicts"));
        }
        self.entities.push(SemanticEntity {
            identity: identity.clone(),
            kind: input.kind.clone(),
            name: input.name.clone(),
            aliases: input.aliases.clone(),
            attributes: input.attributes.clone().unwrap_or_default(),
        });
        Ok(IdentityResolution::Unique(identity))
    }

    pub fn normalize(&mut self) -> Result<()> {
        self.sources
            .sort_by(|a, b| a.source.source_id.cmp(&b.source.source_id));
        self.entities.sort_by(|a, b| a.identity.cmp(&b.identity));
        self.episodes.sort_by(|a, b| a.identity.cmp(&b.identity));
        self.episode_links
            .sort_by(|a, b| a.identity.cmp(&b.identity));
        self.facts.sort_by(|a, b| a.identity.cmp(&b.identity));
        self.mentions.sort_by(|a, b| a.ref_id.cmp(&b.ref_id));
        self.deferred_mentions
            .sort_by(|a, b| a.ref_id.cmp(&b.ref_id));
        let mut history = std::mem::take(&mut self.changes)
            .into_iter()
            .map(|value| Ok((stable_digest(&value)?, value)))
            .collect::<Result<Vec<_>>>()?;
        history.sort_by(|a, b| a.0.cmp(&b.0));
        self.changes = history.into_iter().map(|(_, value)| value).collect();
        let mut keyed = std::mem::take(&mut self.associations)
            .into_iter()
            .map(|value| Ok((stable_digest(&value)?, value)))
            .collect::<Result<Vec<_>>>()?;
        keyed.sort_by(|a, b| a.0.cmp(&b.0));
        self.associations = keyed.into_iter().map(|(_, value)| value).collect();
        Ok(())
    }

    #[expect(
        clippy::too_many_lines,
        reason = "validate the complete semantic catalog and all cross-record references"
    )]
    pub fn validate(&self, context: &ProcessingContext) -> Result<()> {
        context.check_output(
            self.sources.len()
                + self.entities.len()
                + self.episodes.len()
                + self.episode_links.len()
                + self.facts.len()
                + self.associations.len()
                + self.mentions.len()
                + self.deferred_mentions.len()
                + self.changes.len()
                + usize::from(self.last_operation.is_some()),
        )?;
        let mut sources = BTreeMap::new();
        for source in &self.sources {
            context.check()?;
            source.validate()?;
            if sources.insert(&source.source.source_id, source).is_some() {
                return Err(Error::InvalidInput("duplicate semantic source"));
            }
        }
        let mut entities = BTreeSet::new();
        for entity in &self.entities {
            context.check()?;
            entity.validate()?;
            if !entities.insert(&entity.identity) {
                return Err(Error::InvalidInput("duplicate semantic entity identity"));
            }
        }
        let mut episodes = BTreeMap::new();
        for episode in &self.episodes {
            context.check()?;
            episode.validate()?;
            if episodes.insert(&episode.identity, episode).is_some()
                || !sources.contains_key(&episode.source.source_id)
            {
                return Err(Error::InvalidInput(
                    "duplicate episode or missing episode source",
                ));
            }
        }
        let mut facts = BTreeMap::new();
        let mut links = BTreeSet::new();
        for link in &self.episode_links {
            context.check()?;
            link.validate()?;
            if !links.insert(&link.identity)
                || !episodes.contains_key(&link.source)
                || !episodes.contains_key(&link.target)
            {
                return Err(Error::InvalidInput(
                    "duplicate episode link or missing endpoint",
                ));
            }
        }
        for fact in &self.facts {
            context.check()?;
            fact.validate()?;
            if facts.insert(&fact.identity, fact).is_some()
                || !entities.contains(&fact.subject)
                || matches!(&fact.object, super::FactObject::Entity { identity } if !entities.contains(identity))
            {
                return Err(Error::InvalidInput(
                    "duplicate fact or missing semantic endpoint",
                ));
            }
        }
        validate_predecessors(&facts, context)?;
        let mut associations = BTreeSet::new();
        for association in &self.associations {
            context.check()?;
            association.validate()?;
            if let Some(identity) = &association.evidence.episode {
                let episode = episodes
                    .get(identity)
                    .ok_or(Error::InvalidInput("missing source episode"))?;
                if episode.source != association.evidence.source {
                    return Err(Error::InvalidInput("episode source version mismatch"));
                }
                match &association.target {
                    ObjectId::SemanticEntity { identity: target }
                    | ObjectId::Fact { identity: target }
                        if target.space_id == identity.space_id => {}
                    _ => {
                        return Err(Error::InvalidInput(
                            "episode contribution outside identity space",
                        ));
                    }
                }
            }
            let exists = match &association.target {
                ObjectId::SemanticEntity { identity } => entities.contains(identity),
                ObjectId::Fact { identity } => facts.contains_key(identity),
                _ => false,
            };
            let _source = sources
                .get(&association.evidence.source.source_id)
                .ok_or(Error::InvalidInput("missing association source"))?;
            if !exists || !associations.insert(stable_digest(association)?) {
                return Err(Error::InvalidInput(
                    "invalid semantic association target or version",
                ));
            }
        }
        if let Some(operation) = &self.last_operation {
            operation.validate()?;
        }
        self.validate_changes(&facts, &episodes, &sources, context)?;
        let mut mentions = BTreeSet::new();
        for mention in self.mentions.iter().chain(&self.deferred_mentions) {
            context.check()?;
            mention.validate()?;
            if !entities.contains(&mention.target)
                || !mentions.insert(&mention.ref_id)
                || sources
                    .get(&mention.evidence.source_id)
                    .is_none_or(|s| s.semantic_version.is_none())
            {
                return Err(Error::InvalidInput("invalid mention target or source"));
            }
        }
        Ok(())
    }
}

fn validate_predecessors(
    facts: &BTreeMap<&super::Identity, &Fact>,
    context: &ProcessingContext,
) -> Result<()> {
    for fact in facts.values() {
        if let Some(previous) = &fact.supersedes {
            let previous = facts
                .get(previous)
                .ok_or(Error::InvalidInput("missing previous fact"))?;
            if previous.context != fact.context || previous.state == super::FactState::Active {
                return Err(Error::InvalidInput(
                    "invalid previous fact context or state",
                ));
            }
        }
        let mut previous = fact.supersedes.as_ref();
        let mut depth = 0;
        while let Some(id) = previous {
            context.check()?;
            depth += 1;
            if depth > facts.len() {
                return Err(Error::InvalidInput("cyclic fact corrections"));
            }
            previous = facts
                .get(id)
                .ok_or(Error::InvalidInput("missing previous fact"))?
                .supersedes
                .as_ref();
        }
    }
    Ok(())
}
