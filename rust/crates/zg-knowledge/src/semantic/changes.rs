use super::{EpisodeState, FactChange, Identity, SourceEvidence};
use crate::{Error, Result, processing::identifier};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EpisodeWithdrawal {
    pub target: Identity,
    pub expected_state: EpisodeState,
    pub context: String,
    pub changed_at_ms: u64,
    pub evidence: SourceEvidence,
}
impl EpisodeWithdrawal {
    pub fn validate(&self) -> Result<()> {
        self.target.validate()?;
        identifier(&self.context)?;
        self.evidence.validate()?;
        if self.expected_state != EpisodeState::Active {
            return Err(Error::InvalidInput(
                "episode withdrawal requires active state",
            ));
        }
        Ok(())
    }
}

/// Necessary business changes in the current database, not graph snapshots.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, schemars::JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum SemanticChange {
    Fact { change: FactChange },
    Episode { change: EpisodeWithdrawal },
}
impl SemanticChange {
    #[must_use]
    pub fn target(&self) -> &Identity {
        match self {
            Self::Fact { change } => change.target(),
            Self::Episode { change } => &change.target,
        }
    }
    #[must_use]
    pub fn changed_at_ms(&self) -> u64 {
        match self {
            Self::Fact {
                change: FactChange::Supersede { replacement, .. },
            } => replacement.changed_at_ms,
            Self::Fact {
                change: FactChange::Retract { changed_at_ms, .. },
            } => *changed_at_ms,
            Self::Episode { change } => change.changed_at_ms,
        }
    }
    #[must_use]
    pub fn evidence(&self) -> &SourceEvidence {
        match self {
            Self::Fact { change } => change.evidence(),
            Self::Episode { change } => &change.evidence,
        }
    }
    pub fn validate(&self) -> Result<()> {
        match self {
            Self::Fact { change } => change.validate(),
            Self::Episode { change } => change.validate(),
        }
    }
}

/// Only the latest semantic attempt is carried forward. Older build inputs
/// belong to the existing candidate lifecycle, not a second event store.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SemanticOperation {
    pub operation_id: String,
    pub attempt: u64,
    pub base_graph_id: String,
    pub base_facts_digest: String,
    pub input_digest: String,
}
impl SemanticOperation {
    pub fn validate(&self) -> Result<()> {
        identifier(&self.operation_id)?;
        identifier(&self.base_graph_id)?;
        for digest in [&self.base_facts_digest, &self.input_digest] {
            if digest.len() != 64 || !digest.bytes().all(|b| b.is_ascii_hexdigit()) {
                return Err(Error::InvalidInput("invalid semantic operation digest"));
            }
        }
        if self.attempt == 0 {
            return Err(Error::InvalidInput("semantic attempt must be positive"));
        }
        Ok(())
    }
}
