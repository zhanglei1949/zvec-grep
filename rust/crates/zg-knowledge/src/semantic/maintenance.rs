use super::{
    Episode, Fact, FactObject, FactState, IdentityResolution, ObjectId, SemanticBatch,
    SemanticCatalog, SemanticInput, SemanticSource, SemanticSourceState, SemanticUpdate,
    SourceAssociation, SourceChange, SourceEvidence, SourceRef, SourceRole,
};
use crate::{
    Error, Result,
    processing::{ProcessingContext, canonical_json},
};
use std::collections::BTreeSet;

type Contributions = BTreeSet<(SourceRef, ObjectId)>;

impl SemanticCatalog {
    /// Build a complete new directory. Errors never mutate the selected base.
    pub fn apply_sources(
        &self,
        input: &SemanticInput,
        context: &ProcessingContext,
    ) -> Result<Self> {
        let epoch_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|_| Error::InvalidInput("system clock before Unix epoch"))?
            .as_millis();
        self.apply_sources_at(
            input,
            context,
            u64::try_from(epoch_ms).map_err(|_| Error::ResourceExhausted)?,
        )
    }

    /// Persist this operation time with the candidate before import; recovery reuses it.
    pub fn apply_sources_at(
        &self,
        input: &SemanticInput,
        context: &ProcessingContext,
        operation_ms: u64,
    ) -> Result<Self> {
        context.check()?;
        context.check_input(canonical_json(input)?.len())?;
        input.validate()?;
        self.validate(context)?;
        let mut next = self.clone();
        for change in &input.sources {
            context.check()?;
            next.change_source(change)?;
        }
        let mut batches = input
            .sources
            .iter()
            .filter_map(|change| match change {
                SourceChange::Upsert {
                    source,
                    semantic: Some(SemanticUpdate::Complete(batch)),
                } => Some((source, batch)),
                _ => None,
            })
            .collect::<Vec<_>>();
        batches.sort_by_key(|(source, _)| &source.source_id);
        let mut contributions = Contributions::new();
        for &(source, batch) in &batches {
            for (ordinal, entity) in batch.entities.iter().enumerate() {
                context.check()?;
                let IdentityResolution::Unique(identity) =
                    next.register_entity(entity, source, ordinal)?
                else {
                    return Err(Error::Conflict(
                        "semantic entity identity is ambiguous or conflicts",
                    ));
                };
                contributions.insert((source.clone(), ObjectId::SemanticEntity { identity }));
            }
        }
        for &(source, batch) in &batches {
            next.contribute(source, batch, &mut contributions, context, operation_ms)?;
        }
        for episode in &input.episodes {
            context.check()?;
            next.add_episode(episode)?;
        }
        next.support(&batches, contributions)?;
        for link in &input.episode_links {
            link.validate()?;
            if let Some(old) = next
                .episode_links
                .iter()
                .find(|old| old.identity == link.identity)
            {
                if old != link {
                    return Err(Error::Conflict("episode link identity contents conflict"));
                }
            } else {
                next.episode_links.push(link.clone());
            }
        }
        next.apply_corrections(input, &self.associations, context, operation_ms)?;
        next.normalize()?;
        next.validate(context)?;
        Ok(next)
    }

    /// Active facts backed by a current contribution; query authorization is separate.
    pub fn current_facts(&self) -> impl Iterator<Item = &Fact> {
        self.facts.iter().filter(|fact| {
            fact.state == FactState::Active
                && self.associations.iter().any(|a| {
                    matches!(&a.target, ObjectId::Fact { identity } if identity == &fact.identity)
                        && self.effective_support(a)
                })
        })
    }

    /// Shared eligibility rule. Query callers restrict the catalog to authorized contributions first.
    #[must_use]
    pub fn effective_support(&self, association: &SourceAssociation) -> bool {
        association.role == SourceRole::Support
            && self.sources.iter().any(|source| {
                source.state != SemanticSourceState::Deleted
                    && source.source == association.evidence.source
                    && source.semantic_version.as_ref() == Some(&source.source.source_version)
            })
            && association
                .evidence
                .episode
                .as_ref()
                .is_none_or(|identity| {
                    self.episodes.iter().any(|episode| {
                        &episode.identity == identity
                            && episode.source == association.evidence.source
                            && episode.state == super::EpisodeState::Active
                    })
                })
    }

    pub(crate) fn source_episode(&self, source: &SourceRef) -> Result<Option<super::Identity>> {
        let mut episodes = self
            .episodes
            .iter()
            .filter(|episode| &episode.source == source);
        let first = episodes.next().map(|episode| episode.identity.clone());
        if episodes.next().is_some() {
            return Err(Error::Conflict(
                "ambiguous source episode requires explicit evidence binding",
            ));
        }
        Ok(first)
    }

    pub(crate) fn change_source(&mut self, change: &SourceChange) -> Result<()> {
        let source = change.source();
        let existing = self
            .sources
            .iter()
            .position(|s| s.source.source_id == source.source_id);
        if matches!(change, SourceChange::Delete { .. })
            && existing.is_some_and(|i| self.sources[i].source != *source)
        {
            return Err(Error::Conflict("source deletion version conflicts"));
        }
        let (state, semantic_version) = match change {
            SourceChange::Delete { .. } => (SemanticSourceState::Deleted, None),
            SourceChange::Upsert {
                semantic: Some(SemanticUpdate::Complete(_)),
                ..
            } => (
                SemanticSourceState::Complete,
                Some(source.source_version.clone()),
            ),
            SourceChange::Upsert { semantic, .. } => (
                match semantic {
                    Some(SemanticUpdate::Partial(_)) => SemanticSourceState::Partial,
                    Some(SemanticUpdate::Failed { .. }) => SemanticSourceState::Failed,
                    _ => SemanticSourceState::Unspecified,
                },
                existing.and_then(|i| self.sources[i].semantic_version.clone()),
            ),
        };
        if change.replaces_semantics() {
            self.withdraw(&source.source_id);
        }
        let record = SemanticSource {
            source: source.clone(),
            state,
            semantic_version,
        };
        if let Some(index) = existing {
            self.sources[index] = record;
        } else {
            self.sources.push(record);
        }
        Ok(())
    }

    fn withdraw(&mut self, source: &str) {
        let mut retained = Vec::new();
        for mut association in std::mem::take(&mut self.associations) {
            if association.evidence.source.source_id == source
                && matches!(
                    association.role,
                    SourceRole::Support | SourceRole::Correction
                )
            {
                association.role = SourceRole::Withdrawn;
            }
            if !retained.contains(&association) {
                retained.push(association);
            }
        }
        self.associations = retained;
        self.mentions
            .retain(|mention| mention.evidence.source_id != source);
        self.deferred_mentions
            .retain(|mention| mention.evidence.source_id != source);
    }

    fn contribute(
        &mut self,
        source: &SourceRef,
        batch: &SemanticBatch,
        contributions: &mut Contributions,
        context: &ProcessingContext,
        operation_ms: u64,
    ) -> Result<()> {
        for fact in &batch.facts {
            context.check()?;
            if let Some(existing) = self.facts.iter().find(|f| f.identity == fact.identity) {
                if existing.subject != fact.subject
                    || existing.relation_type != fact.relation_type
                    || existing.object != fact.object
                    || existing.context != fact.context
                    || existing.attributes != fact.attributes
                {
                    return Err(Error::Conflict("semantic fact identity contents conflict"));
                }
                // Source replay cannot rewrite a business correction or its timestamp.
            } else {
                let mut fact = fact.clone();
                fact.created_at_ms = Some(operation_ms);
                fact.changed_at_ms = operation_ms;
                self.facts.push(fact);
            }
            contributions.insert((
                source.clone(),
                ObjectId::Fact {
                    identity: fact.identity.clone(),
                },
            ));
            contributions.insert((
                source.clone(),
                ObjectId::SemanticEntity {
                    identity: fact.subject.clone(),
                },
            ));
            if let FactObject::Entity { identity } = &fact.object {
                contributions.insert((
                    source.clone(),
                    ObjectId::SemanticEntity {
                        identity: identity.clone(),
                    },
                ));
            }
        }
        for mention in &batch.mentions {
            context.check()?;
            self.mentions.push(mention.clone());
            contributions.insert((
                source.clone(),
                ObjectId::SemanticEntity {
                    identity: mention.target.clone(),
                },
            ));
        }
        Ok(())
    }

    fn support(
        &mut self,
        batches: &[(&SourceRef, &SemanticBatch)],
        contributions: Contributions,
    ) -> Result<()> {
        for (source, batch) in batches {
            for mention in &batch.mentions {
                let association = SourceAssociation {
                    target: ObjectId::SemanticEntity {
                        identity: mention.target.clone(),
                    },
                    evidence: SourceEvidence {
                        episode: self.source_episode(source)?,
                        source: (*source).clone(),
                        locator: Some(mention.evidence.clone()),
                    },
                    role: SourceRole::Support,
                };
                if !batch.associations.contains(&association) {
                    self.associations.push(association);
                }
            }
            for association in &batch.associations {
                self.associations.push(association.clone());
            }
        }
        let mut supported: Contributions = self
            .associations
            .iter()
            .filter(|a| a.role == SourceRole::Support)
            .map(|a| (a.evidence.source.clone(), a.target.clone()))
            .collect();
        for (source, target) in contributions {
            if supported.insert((source.clone(), target.clone())) {
                self.associations.push(SourceAssociation {
                    target,
                    evidence: SourceEvidence {
                        episode: self.source_episode(&source)?,
                        source: source.clone(),
                        locator: None,
                    },
                    role: SourceRole::Support,
                });
            }
        }
        let live = self
            .associations
            .iter()
            .filter(|a| a.role == SourceRole::Support)
            .map(|a| (a.target.clone(), a.evidence.clone()))
            .collect::<Vec<_>>();
        self.associations.retain(|a| {
            a.role != SourceRole::Withdrawn
                || !live.contains(&(a.target.clone(), a.evidence.clone()))
        });
        Ok(())
    }

    fn add_episode(&mut self, episode: &Episode) -> Result<()> {
        let source = self
            .sources
            .iter()
            .find(|s| s.source.source_id == episode.source.source_id)
            .ok_or(Error::InvalidInput("missing episode source"))?;
        if source.source != episode.source || source.state == SemanticSourceState::Deleted {
            return Err(Error::Conflict("episode source version conflicts"));
        }
        if let Some(existing) = self
            .episodes
            .iter()
            .find(|e| e.identity == episode.identity)
        {
            if existing != episode {
                return Err(Error::Conflict("episode identity contents conflict"));
            }
        } else {
            if episode.state != super::EpisodeState::Active {
                return Err(Error::InvalidInput(
                    "new episodes must start active; use episode withdrawal",
                ));
            }
            self.episodes.push(episode.clone());
        }
        Ok(())
    }
}
