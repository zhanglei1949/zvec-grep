//! Immutable graph build input shared by builders and leased readers.
use serde::{Deserialize, Serialize};
use zg_knowledge::graph::{
    ImportBudget,
    build::{BuildScope, BuildSpec},
};

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct BuildRecord {
    pub(crate) format_version: u32,
    pub(crate) graph_build_id: String,
    pub(crate) operation_id: String,
    pub(crate) attempt: u64,
    pub(crate) base_text_revision: u64,
    pub(crate) base_catalog_digest: String,
    pub(crate) base_head: Option<String>,
    pub(crate) scope: BuildScope,
    pub(crate) configuration: serde_json::Value,
    pub(crate) policy_fingerprint: String,
    pub(crate) rules_fingerprint: String,
    pub(crate) module_roots_fingerprint: String,
    pub(crate) schema_fingerprint: String,
    pub(crate) created_epoch_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) import_budget: Option<ImportBudget>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) semantic: Option<SemanticBuildInput>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) configuration_inputs:
        Vec<crate::graph_facts::module_roots::configuration::ConfigurationInput>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct SemanticBuildInput {
    pub(crate) base_facts_digest: String,
    pub(crate) input: zg_knowledge::semantic::SemanticInput,
}
impl BuildRecord {
    pub(crate) fn semantic_operation(
        &self,
    ) -> EngineResult<Option<zg_knowledge::semantic::SemanticOperation>> {
        self.semantic
            .as_ref()
            .map(|semantic| {
                if self.graph_build_id != self.operation_id || self.attempt != 1 {
                    return Err(crate::EngineError::invalid_argument(
                        "invalid semantic attempt identity",
                    ));
                }
                semantic
                    .input
                    .validate()
                    .map_err(crate::graph_facts::map_error)?;
                let operation = zg_knowledge::semantic::SemanticOperation {
                    operation_id: self.operation_id.clone(),
                    attempt: self.attempt,
                    base_graph_id: self.base_head.clone().ok_or_else(|| {
                        crate::EngineError::invalid_argument("semantic build has no base")
                    })?,
                    base_facts_digest: semantic.base_facts_digest.clone(),
                    input_digest: fingerprint(&semantic.input)?,
                };
                operation
                    .validate()
                    .map_err(crate::graph_facts::map_error)?;
                Ok(operation)
            })
            .transpose()
    }
    pub(crate) fn expected_rules_fingerprint(&self) -> EngineResult<String> {
        let rules = if self.configuration_inputs.is_empty() {
            rules_fingerprint()?
        } else {
            fingerprint(&(rules_fingerprint()?, &self.configuration_inputs))?
        };
        match self.semantic_operation()? {
            Some(operation) => fingerprint(&(rules, operation)),
            None => Ok(rules),
        }
    }
    pub(crate) fn spec(&self) -> BuildSpec {
        BuildSpec {
            graph_build_id: self.graph_build_id.clone(),
            operation_id: self.operation_id.clone(),
            attempt: self.attempt,
            base_text_revision: self.base_text_revision,
            base_catalog_digest: self.base_catalog_digest.clone(),
            base_head: self.base_head.clone(),
            scope: self.scope.clone(),
            configuration: self.configuration.clone(),
            policy_fingerprint: self.policy_fingerprint.clone(),
            rules_fingerprint: self.rules_fingerprint.clone(),
            module_roots_fingerprint: self.module_roots_fingerprint.clone(),
        }
    }
}

use super::fingerprint;
use crate::{EngineResult, domain::FileRecord, storage::IndexStore};
use zg_knowledge::relations::bound::{FactCatalog, entity_ids_digest};

// Bump the corresponding component when parser/binding/resolution semantics
// change. Schema and caller policy are separate compatibility dimensions.
pub(crate) fn rules_fingerprint() -> EngineResult<String> {
    fingerprint(&(
        zg_knowledge::relations::FACT_CONTRACT_VERSION,
        "host-binding-v1",
        "static-code-v8",
        "commonmark-links-v1",
        "source-coverage-v2",
        "business-model-v4",
        "text-map-v2",
    ))
}

/// Validate the real inventory and every retained file's actual `EntityIds`.
pub(crate) fn bound_matches(
    storage: &IndexStore,
    files: &[FileRecord],
    catalog: &FactCatalog,
) -> EngineResult<bool> {
    for bound in &catalog.files {
        let Some(file) = files.iter().find(|f| {
            f.id.get() == bound.record.file_id
                && f.relative_path.as_ref() == std::path::Path::new(&bound.record.path)
        }) else {
            return Ok(false);
        };
        if !file.index_status.is_indexed()
            || file.snapshot.content_hash != bound.record.content_hash
            || file.snapshot.size_bytes != bound.record.size_bytes
        {
            return Ok(false);
        }
        let ids = storage.file_entity_ids(file)?;
        if entity_ids_digest(&ids).map_err(crate::graph_facts::map_error)?
            != bound.record.entity_ids_digest
        {
            return Ok(false);
        }
        if bound.facts.is_some()
            && !crate::graph_facts::text_binding::verify(
                storage,
                &ids,
                &bound.record.path,
                &bound.text_map,
                bound.text_binding_digest.as_deref(),
            )?
        {
            return Ok(false);
        }
    }
    Ok(true)
}
