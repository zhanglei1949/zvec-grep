use super::{
    EntityInput, Episode, Fact, FactState, Identity, Mention, ObjectId, SourceAssociation,
    SourceEvidence, SourceRef, same_space,
};
use crate::{Error, Result, processing::identifier};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SemanticBatch {
    pub entities: Vec<EntityInput>,
    pub facts: Vec<Fact>,
    pub mentions: Vec<Mention>,
    pub associations: Vec<SourceAssociation>,
}
impl SemanticBatch {
    pub fn validate(&self, source: &SourceRef, space_id: &str) -> Result<()> {
        let mut entities = BTreeSet::new();
        for entity in &self.entities {
            entity.validate()?;
            same_space(&entity.space_id, space_id)?;
            if let Some(id) = &entity.id
                && !entities.insert(id)
            {
                return Err(Error::InvalidInput("duplicate entity ID within source"));
            }
        }
        let mut facts = BTreeSet::new();
        for fact in &self.facts {
            fact.validate()?;
            same_space(&fact.identity.space_id, space_id)?;
            if !facts.insert(&fact.identity)
                || fact.state != FactState::Active
                || fact.supersedes.is_some()
            {
                return Err(Error::InvalidInput(
                    "new facts require unique IDs and active state without correction",
                ));
            }
        }
        let mut mentions = BTreeSet::new();
        for mention in &self.mentions {
            mention.validate()?;
            same_space(&mention.target.space_id, space_id)?;
            if !mentions.insert(&mention.ref_id) {
                return Err(Error::InvalidInput("duplicate mention ID"));
            }
            if mention.evidence.source_id != source.source_id {
                return Err(Error::InvalidInput("mention source mismatch"));
            }
        }
        for association in &self.associations {
            association.validate()?;
            if association.role != super::SourceRole::Support {
                return Err(Error::InvalidInput(
                    "source batches only declare current support",
                ));
            }
            if let ObjectId::SemanticEntity { identity } | ObjectId::Fact { identity } =
                &association.target
            {
                same_space(&identity.space_id, space_id)?;
            }
            if &association.evidence.source != source {
                return Err(Error::InvalidInput("association source version mismatch"));
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize, schemars::JsonSchema)]
#[serde(
    tag = "status",
    content = "value",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum SemanticUpdate {
    Complete(SemanticBatch),
    Partial(SemanticBatch),
    Failed { reason: String },
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize, schemars::JsonSchema)]
#[serde(tag = "change", rename_all = "snake_case", deny_unknown_fields)]
pub enum SourceChange {
    Upsert {
        source: SourceRef,
        semantic: Option<SemanticUpdate>,
    },
    Delete {
        source: SourceRef,
    },
}
impl SourceChange {
    #[must_use]
    pub fn source(&self) -> &SourceRef {
        match self {
            Self::Upsert { source, .. } | Self::Delete { source } => source,
        }
    }
    #[must_use]
    pub fn replaces_semantics(&self) -> bool {
        matches!(
            self,
            Self::Upsert {
                semantic: Some(SemanticUpdate::Complete(_)),
                ..
            } | Self::Delete { .. }
        )
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, schemars::JsonSchema)]
#[serde(tag = "change", rename_all = "snake_case", deny_unknown_fields)]
pub enum FactChange {
    Supersede {
        target: Identity,
        expected_state: FactState,
        replacement: Box<Fact>,
        evidence: SourceEvidence,
    },
    Retract {
        target: Identity,
        expected_state: FactState,
        context: String,
        changed_at_ms: u64,
        evidence: SourceEvidence,
    },
}
impl FactChange {
    #[must_use]
    pub fn evidence(&self) -> &SourceEvidence {
        match self {
            Self::Supersede { evidence, .. } | Self::Retract { evidence, .. } => evidence,
        }
    }
    #[must_use]
    pub fn target(&self) -> &Identity {
        match self {
            Self::Supersede { target, .. } | Self::Retract { target, .. } => target,
        }
    }
    pub fn validate(&self) -> Result<()> {
        self.target().validate()?;
        match self {
            Self::Supersede {
                target,
                expected_state,
                replacement,
                evidence,
            } => {
                replacement.validate()?;
                evidence.validate()?;
                same_space(&replacement.identity.space_id, &target.space_id)?;
                if *expected_state != FactState::Active
                    || replacement.state != FactState::Active
                    || replacement.supersedes.as_ref() != Some(target)
                {
                    return Err(Error::InvalidInput(
                        "invalid fact replacement state or predecessor",
                    ));
                }
            }
            Self::Retract {
                context,
                evidence,
                expected_state,
                ..
            } => {
                identifier(context)?;
                evidence.validate()?;
                if *expected_state == FactState::Retracted {
                    return Err(Error::InvalidInput("fact is already retracted"));
                }
            }
        }
        Ok(())
    }
    /// Input and current fact have already passed validation at their boundaries.
    pub fn check_current(&self, current: &Fact) -> Result<()> {
        let (expected, context) = match self {
            Self::Supersede {
                expected_state,
                replacement,
                ..
            } => (*expected_state, replacement.context.as_str()),
            Self::Retract {
                expected_state,
                context,
                ..
            } => (*expected_state, context.as_str()),
        };
        if self.target() != &current.identity
            || expected != current.state
            || context != current.context
        {
            return Err(Error::Conflict(
                "fact target, context or expected state conflicts",
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SemanticInput {
    pub space_id: String,
    pub update_scope: Vec<String>,
    pub sources: Vec<SourceChange>,
    pub episodes: Vec<Episode>,
    pub fact_changes: Vec<FactChange>,
    #[serde(default)]
    pub episode_withdrawals: Vec<super::EpisodeWithdrawal>,
}
impl SemanticInput {
    pub fn validate(&self) -> Result<()> {
        identifier(&self.space_id)?;
        let mut scope = BTreeSet::new();
        for id in &self.update_scope {
            identifier(id)?;
            if !scope.insert(id) {
                return Err(Error::InvalidInput("duplicate update source"));
            }
        }
        let mut sources = BTreeMap::new();
        for change in &self.sources {
            let source = change.source();
            source.validate()?;
            if !scope.contains(&source.source_id)
                || sources.insert(&source.source_id, source).is_some()
            {
                return Err(Error::InvalidInput(
                    "source outside update scope or duplicated",
                ));
            }
            if let SourceChange::Upsert {
                semantic: Some(update),
                ..
            } = change
            {
                match update {
                    SemanticUpdate::Complete(batch) | SemanticUpdate::Partial(batch) => {
                        batch.validate(source, &self.space_id)?;
                    }
                    SemanticUpdate::Failed { reason } => identifier(reason)?,
                }
            }
        }
        let mut episodes = BTreeSet::new();
        for episode in &self.episodes {
            episode.validate()?;
            same_space(&episode.identity.space_id, &self.space_id)?;
            if !scope.contains(&episode.source.source_id) || !episodes.insert(&episode.identity) {
                return Err(Error::InvalidInput("episode outside scope or duplicated"));
            }
            if sources
                .get(&episode.source.source_id)
                .is_some_and(|source| **source != episode.source)
            {
                return Err(Error::InvalidInput("episode source version mismatch"));
            }
        }
        let mut targets = BTreeSet::new();
        for change in &self.fact_changes {
            change.validate()?;
            same_space(&change.target().space_id, &self.space_id)?;
            if !scope.contains(&change.evidence().source.source_id)
                || !targets.insert(change.target())
            {
                return Err(Error::InvalidInput(
                    "fact change source outside scope or target duplicated",
                ));
            }
            let evidence = &change.evidence().source;
            if sources
                .get(&evidence.source_id)
                .is_some_and(|source| *source != evidence)
            {
                return Err(Error::InvalidInput("fact change source version mismatch"));
            }
        }
        let mut withdrawn = BTreeSet::new();
        for change in &self.episode_withdrawals {
            change.validate()?;
            same_space(&change.target.space_id, &self.space_id)?;
            if !scope.contains(&change.evidence.source.source_id)
                || !withdrawn.insert(&change.target)
                || episodes.contains(&change.target)
            {
                return Err(Error::InvalidInput(
                    "episode withdrawal outside scope or duplicated",
                ));
            }
            if sources
                .get(&change.evidence.source.source_id)
                .is_some_and(|source| *source != &change.evidence.source)
            {
                return Err(Error::InvalidInput(
                    "episode withdrawal source version mismatch",
                ));
            }
        }
        Ok(())
    }
}
