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
            next.contribute(source, batch, &mut contributions, context)?;
        }
        next.support(&batches, contributions);
        for episode in &input.episodes {
            context.check()?;
            next.add_episode(episode)?;
        }
        next.apply_corrections(input, &self.associations, context)?;
        next.normalize()?;
        next.validate(context)?;
        Ok(next)
    }

    /// Active facts backed by a current contribution; query authorization is separate.
    pub fn current_facts(&self) -> impl Iterator<Item = &Fact> {
        let mut supported: BTreeSet<_> = self
            .associations
            .iter()
            .filter_map(|a| {
                if a.role == SourceRole::Support
                    && let ObjectId::Fact { identity } = &a.target
                {
                    Some(identity)
                } else {
                    None
                }
            })
            .collect();
        let live_sources: BTreeSet<_> = self
            .sources
            .iter()
            .filter(|s| s.state != SemanticSourceState::Deleted)
            .map(|s| &s.source.source_id)
            .collect();
        let mut corrections: std::collections::BTreeMap<_, Vec<_>> =
            std::collections::BTreeMap::new();
        for association in self.associations.iter().filter(|a| {
            a.role == SourceRole::Correction && live_sources.contains(&a.evidence.source.source_id)
        }) {
            if let ObjectId::Fact { identity } = &association.target {
                corrections
                    .entry((identity, &association.evidence.source))
                    .or_default()
                    .push(&association.evidence);
            }
        }
        for change in &self.changes {
            if let super::SemanticChange::Fact {
                change:
                    super::FactChange::Supersede {
                        replacement,
                        evidence,
                        ..
                    },
            } = change
                && corrections
                    .get(&(&replacement.identity, &evidence.source))
                    .is_some_and(|items| items.contains(&evidence))
            {
                supported.insert(&replacement.identity);
            }
        }
        self.facts.iter().filter(move |fact| {
            fact.state == FactState::Active && supported.contains(&fact.identity)
        })
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
        let targets: BTreeSet<_> = self
            .associations
            .iter()
            .filter(|a| {
                matches!(a.role, SourceRole::Support | SourceRole::Correction)
                    && a.evidence.source.source_id == source
            })
            .map(|a| a.target.clone())
            .collect();
        self.associations.retain(|a| {
            !(a.role == SourceRole::Withdrawn
                && a.evidence.source.source_id == source
                && targets.contains(&a.target))
        });
        for association in &mut self.associations {
            if matches!(
                association.role,
                SourceRole::Support | SourceRole::Correction
            ) && association.evidence.source.source_id == source
            {
                association.role = SourceRole::Withdrawn;
            }
        }
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
    ) -> Result<()> {
        for fact in &batch.facts {
            context.check()?;
            if let Some(existing) = self.facts.iter().find(|f| f.identity == fact.identity) {
                if existing.subject != fact.subject
                    || existing.predicate != fact.predicate
                    || existing.object != fact.object
                    || existing.context != fact.context
                    || existing.attributes != fact.attributes
                {
                    return Err(Error::Conflict("semantic fact identity contents conflict"));
                }
                // Source replay cannot rewrite a business correction or its timestamp.
            } else {
                self.facts.push(fact.clone());
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

    fn support(&mut self, batches: &[(&SourceRef, &SemanticBatch)], contributions: Contributions) {
        for (source, batch) in batches {
            for mention in &batch.mentions {
                let association = SourceAssociation {
                    target: ObjectId::SemanticEntity {
                        identity: mention.target.clone(),
                    },
                    evidence: SourceEvidence {
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
                        source,
                        locator: None,
                    },
                    role: SourceRole::Support,
                });
            }
        }
        let current: BTreeSet<_> = supported.iter().map(|(s, t)| (&s.source_id, t)).collect();
        self.associations.retain(|a| {
            a.role != SourceRole::Withdrawn
                || !current.contains(&(&a.evidence.source.source_id, &a.target))
        });
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
