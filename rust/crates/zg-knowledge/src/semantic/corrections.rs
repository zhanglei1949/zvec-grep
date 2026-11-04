use super::{
    EpisodeState, FactChange, FactState, ObjectId, SemanticCatalog, SemanticChange, SemanticInput,
    SemanticSourceState, SourceAssociation, SourceEvidence, SourceRole,
};
use crate::{Error, Result, processing::ProcessingContext};

impl SemanticCatalog {
    pub(super) fn apply_corrections(
        &mut self,
        input: &SemanticInput,
        previous_associations: &[SourceAssociation],
        context: &ProcessingContext,
    ) -> Result<()> {
        for change in &input.fact_changes {
            context.check()?;
            let record = SemanticChange::Fact {
                change: change.clone(),
            };
            if self.changes.contains(&record) {
                // Replacing the same complete batch preserves only correction
                // support that was live before this call, never withdrawn history.
                let replacement = match change {
                    FactChange::Supersede { replacement, .. } => Some(&replacement.identity),
                    FactChange::Retract { .. } => None,
                };
                for identity in std::iter::once(change.target()).chain(replacement) {
                    if self.sources.iter().any(|s| {
                        s.source == change.evidence().source
                            && s.state != SemanticSourceState::Deleted
                    }) && previous_associations.iter().any(|a| {
                        a.role == SourceRole::Correction
                            && a.evidence == *change.evidence()
                            && matches!(&a.target, ObjectId::Fact { identity: target } if target == identity)
                    }) {
                        self.correction_association(identity, change.evidence());
                    }
                }
                continue;
            }
            self.correction_source(change.evidence())?;
            let index = self
                .facts
                .iter()
                .position(|f| &f.identity == change.target())
                .ok_or(Error::Conflict("fact correction target missing"))?;
            change.check_current(&self.facts[index])?;
            match change {
                FactChange::Supersede {
                    replacement,
                    evidence,
                    ..
                } => {
                    if self
                        .facts
                        .iter()
                        .any(|f| f.identity == replacement.identity)
                    {
                        return Err(Error::Conflict("replacement fact identity already exists"));
                    }
                    self.facts[index].state = FactState::Superseded;
                    self.facts[index].changed_at_ms = replacement.changed_at_ms;
                    self.facts.push((**replacement).clone());
                    self.correction_association(&replacement.identity, evidence);
                }
                FactChange::Retract { changed_at_ms, .. } => {
                    self.facts[index].state = FactState::Retracted;
                    self.facts[index].changed_at_ms = *changed_at_ms;
                }
            }
            self.correction_association(change.target(), change.evidence());
            self.changes.push(record);
        }
        for change in &input.episode_withdrawals {
            context.check()?;
            let record = SemanticChange::Episode {
                change: change.clone(),
            };
            if self.changes.contains(&record) {
                continue;
            }
            self.correction_source(&change.evidence)?;
            let episode = self
                .episodes
                .iter_mut()
                .find(|e| e.identity == change.target)
                .ok_or(Error::Conflict("episode withdrawal target missing"))?;
            if episode.state != change.expected_state || episode.context != change.context {
                return Err(Error::Conflict(
                    "episode context or expected state conflicts",
                ));
            }
            episode.state = EpisodeState::Withdrawn;
            self.changes.push(record);
        }
        Ok(())
    }
    fn correction_source(&self, evidence: &SourceEvidence) -> Result<()> {
        if !self.sources.iter().any(|s| s.source == evidence.source) {
            return Err(Error::Conflict("correction source version conflicts"));
        }
        Ok(())
    }
    fn correction_association(&mut self, identity: &super::Identity, evidence: &SourceEvidence) {
        let deleted = self.sources.iter().any(|s| {
            s.source.source_id == evidence.source.source_id
                && s.state == SemanticSourceState::Deleted
        });
        let association = SourceAssociation {
            target: ObjectId::Fact {
                identity: identity.clone(),
            },
            evidence: evidence.clone(),
            role: if deleted {
                SourceRole::Withdrawn
            } else {
                SourceRole::Correction
            },
        };
        if !self.associations.contains(&association) {
            self.associations.push(association);
        }
    }
}

impl SemanticCatalog {
    pub(super) fn validate_changes(
        &self,
        facts: &std::collections::BTreeMap<&super::Identity, &super::Fact>,
        episodes: &std::collections::BTreeMap<&super::Identity, &super::Episode>,
        sources: &std::collections::BTreeMap<&String, &super::SemanticSource>,
        context: &ProcessingContext,
    ) -> Result<()> {
        let mut receipts = std::collections::BTreeSet::new();
        for record in &self.changes {
            context.check()?;
            record.validate()?;
            if !receipts.insert(crate::processing::stable_digest(record)?)
                || !sources.contains_key(&record.evidence().source.source_id)
            {
                return Err(Error::InvalidInput(
                    "duplicate change or missing historical source",
                ));
            }
            match record {
                SemanticChange::Fact { change } => {
                    let old = facts
                        .get(change.target())
                        .ok_or(Error::InvalidInput("missing changed fact"))?;
                    match change {
                        FactChange::Supersede { replacement, .. } => {
                            let new = facts
                                .get(&replacement.identity)
                                .ok_or(Error::InvalidInput("missing replacement fact"))?;
                            if old.state == FactState::Active
                                || old.context != replacement.context
                                || new.subject != replacement.subject
                                || new.predicate != replacement.predicate
                                || new.object != replacement.object
                                || new.context != replacement.context
                                || new.attributes != replacement.attributes
                                || new.supersedes != replacement.supersedes
                            {
                                return Err(Error::InvalidInput("inconsistent stored replacement"));
                            }
                        }
                        FactChange::Retract { context, .. } => {
                            if old.state != FactState::Retracted || old.context != *context {
                                return Err(Error::InvalidInput("inconsistent stored retraction"));
                            }
                        }
                    }
                }
                SemanticChange::Episode { change } => {
                    if episodes.get(&change.target).is_none_or(|e| {
                        e.context != change.context || e.state != EpisodeState::Withdrawn
                    }) {
                        return Err(Error::InvalidInput("inconsistent episode withdrawal"));
                    }
                }
            }
        }
        Ok(())
    }
}
