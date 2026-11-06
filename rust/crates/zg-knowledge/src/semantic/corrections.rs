use super::{
    EpisodeState, FactChange, FactState, ObjectId, SemanticCatalog, SemanticChange, SemanticInput,
    SemanticSourceState, SourceAssociation, SourceEvidence, SourceRole,
};
use crate::{Error, Result, processing::ProcessingContext};

impl SemanticCatalog {
    #[expect(
        clippy::too_many_lines,
        reason = "replay and correction changes form one candidate operation"
    )]
    pub(super) fn apply_corrections(
        &mut self,
        input: &SemanticInput,
        previous_associations: &[SourceAssociation],
        context: &ProcessingContext,
        operation_ms: u64,
    ) -> Result<()> {
        for change in &input.fact_changes {
            context.check()?;
            let mut normalized = change.clone();
            let evidence = match &mut normalized {
                FactChange::Supersede { evidence, .. } | FactChange::Retract { evidence, .. } => {
                    evidence
                }
            };
            if evidence.episode.is_none() {
                evidence.episode = self.source_episode(&evidence.source)?;
            }
            let change = &normalized;
            let record = SemanticChange::Fact {
                change: change.clone(),
            };
            if self.changes.contains(&record) {
                // Replays may restore a contribution replaced by this same complete batch,
                // but never a contribution withdrawn before the operation.
                for association in previous_associations
                    .iter()
                    .filter(|a| a.evidence == *change.evidence())
                {
                    if matches!(
                        association.role,
                        SourceRole::Support | SourceRole::Correction
                    ) && self.sources.iter().any(|s| {
                        s.source == association.evidence.source
                            && s.state != SemanticSourceState::Deleted
                    }) && !self.associations.contains(association)
                    {
                        self.associations.retain(|a| {
                            !(a.role == SourceRole::Withdrawn
                                && a.target == association.target
                                && a.evidence == association.evidence)
                        });
                        self.associations.push(association.clone());
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
                    self.facts[index].changed_at_ms = operation_ms;
                    let mut new_fact = (**replacement).clone();
                    new_fact.created_at_ms = Some(operation_ms);
                    new_fact.changed_at_ms = operation_ms;
                    self.facts.push(new_fact);
                    let mut evidence = evidence.clone();
                    if evidence.episode.is_none() {
                        evidence.episode = self.source_episode(&evidence.source)?;
                    }
                    let association = SourceAssociation {
                        target: ObjectId::Fact {
                            identity: replacement.identity.clone(),
                        },
                        evidence,
                        role: SourceRole::Support,
                    };
                    for identity in
                        std::iter::once(&replacement.subject).chain(match &replacement.object {
                            super::FactObject::Entity { identity } => Some(identity),
                            super::FactObject::Scalar { .. } => None,
                        })
                    {
                        let mut endpoint = association.clone();
                        endpoint.target = ObjectId::SemanticEntity {
                            identity: identity.clone(),
                        };
                        if !self.associations.contains(&endpoint) {
                            self.associations.push(endpoint);
                        }
                    }
                    if !self.associations.contains(&association) {
                        self.associations.push(association);
                    }
                }
                FactChange::Retract { .. } => {
                    self.facts[index].state = FactState::Retracted;
                    self.facts[index].changed_at_ms = operation_ms;
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
    fn correction_source(&mut self, evidence: &SourceEvidence) -> Result<()> {
        let source = self
            .sources
            .iter_mut()
            .find(|s| s.source == evidence.source && s.state != SemanticSourceState::Deleted)
            .ok_or(Error::Conflict("correction source version conflicts"))?;
        // An explicit, validated correction verifies this version even when the
        // caller did not replace all semantic contributions from the source.
        source.semantic_version = Some(source.source.source_version.clone());
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
                                || new.relation_type != replacement.relation_type
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
