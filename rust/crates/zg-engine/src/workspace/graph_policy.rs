//! Workspace intent survives unsuccessful candidate generation cleanup.
use crate::storage::graph_state::{fingerprint, now, read_record, write_record};
use crate::{EngineError, EngineResult, api::index::graph::GraphOptions};
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct GraphPolicy {
    pub format_version: u32,
    pub configuration: GraphOptions,
    pub policy_fingerprint: String,
    pub updated_epoch_ms: u64,
}
impl GraphPolicy {
    pub fn read(home: &Path) -> EngineResult<Option<Self>> {
        let policy: Option<Self> = read_record(&home.join("graph/POLICY.json"))?;
        if let Some(p) = &policy
            && fingerprint(&p.configuration)? != p.policy_fingerprint
        {
            return Err(EngineError::storage_failure(
                "graph policy fingerprint mismatch",
            ));
        }
        Ok(policy)
    }
    pub fn save(home: &Path, configuration: GraphOptions) -> EngineResult<Self> {
        // Never overwrite a format that this binary cannot read.
        Self::read(home)?;
        let policy = Self {
            format_version: 1,
            policy_fingerprint: fingerprint(&configuration)?,
            configuration,
            updated_epoch_ms: now(),
        };
        let path = home.join("graph/POLICY.json");
        if let Err(error) = write_record(&path, &policy) {
            if Self::read(home).ok().flatten().as_ref() != Some(&policy) {
                return Err(error);
            }
            // A rename may have happened but durability is unconfirmed. Do not
            // admit subsequent writes until another durable write succeeds.
            write_record(&path, &policy)?;
        }
        Ok(policy)
    }
}
