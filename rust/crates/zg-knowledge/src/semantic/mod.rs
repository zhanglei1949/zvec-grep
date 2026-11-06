//! Shared semantic contracts. Persistence and host publication are separate.
//! Source nodes continue to use the existing source facts and `EntityRecord`.
mod catalog;
mod changes;
mod corrections;
mod identity;
mod input;
mod maintenance;
mod records;

pub use catalog::{SemanticCatalog, SemanticSource, SemanticSourceState};
pub use changes::{EpisodeWithdrawal, SemanticChange, SemanticOperation};
pub use identity::{EntityInput, Identity, IdentityResolution, ObjectId, resolve_identity};
pub use input::{FactChange, SemanticBatch, SemanticInput, SemanticUpdate, SourceChange};
pub use records::{
    Episode, EpisodeLink, EpisodeRelation, EpisodeState, Fact, FactObject, FactState, Mention,
    ScalarValue, SemanticEntity, SourceAssociation, SourceEvidence, SourceRef, SourceRole,
};

use crate::{Error, Result};

fn same_space(actual: &str, expected: &str) -> Result<()> {
    if actual != expected {
        return Err(Error::InvalidInput("semantic identity space mismatch"));
    }
    Ok(())
}
