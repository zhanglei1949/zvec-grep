//! One selected publication binds logical identity to text and graph artifacts.
use super::manifest::WorkspaceManifest;
use crate::{
    EngineError, EngineResult,
    storage::graph_state::{self, GraphHead, TextPhase, TextState},
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Publication {
    pub generation: String,
    pub knowledge_base_id: String,
    pub storage_generation: String,
    pub text_revision: u64,
    pub catalog_digest: String,
    pub configuration_digest: String,
    pub graph: Option<GraphHead>,
    pub graph_ready: bool,
}
impl Publication {
    pub fn validate(&self, storage: Option<&str>) -> EngineResult<()> {
        if !graph_state::valid_uuid(&self.knowledge_base_id)
            || !graph_state::valid_uuid(&self.generation)
            || Some(self.storage_generation.as_str()) != storage
            || !graph_state::valid_digest(&self.catalog_digest)
            || !graph_state::valid_digest(&self.configuration_digest)
        {
            return Err(EngineError::storage_failure("invalid publication identity"));
        }
        if let Some(graph) = &self.graph {
            graph.validate_lifecycle()?;
            if self.graph_ready
                && (graph.text_revision != self.text_revision
                    || graph.catalog_digest != self.catalog_digest)
            {
                return Err(EngineError::storage_failure(
                    "publication graph/text mismatch",
                ));
            }
        }
        if self.graph_ready && self.graph.is_none() {
            return Err(EngineError::storage_failure(
                "ready publication has no graph",
            ));
        }
        Ok(())
    }
}

/// A manifest rollback cannot undo writes in its selected physical directory.
pub(crate) fn require_readable_text(manifest: &WorkspaceManifest) -> EngineResult<()> {
    let state = TextState::read(&manifest.storage_home())?;
    if state.as_ref().is_some_and(|s| s.state != TextPhase::Stable) {
        return Err(EngineError::recovery_required(
            "text mutation must be reconciled before reading",
        ));
    }
    if let Some(publication) = &manifest.publication {
        let revision = state.as_ref().map_or(0, |s| s.text_revision);
        let digest = state.as_ref().map_or_else(
            || graph_state::fingerprint(&Vec::<String>::new()),
            |s| Ok(s.catalog_digest.clone()),
        )?;
        if publication.text_revision != revision || publication.catalog_digest != digest {
            return Err(EngineError::recovery_required(
                "published text version does not match its physical storage",
            ));
        }
    }
    Ok(())
}

pub(crate) fn snapshot(
    manifest: &WorkspaceManifest,
    graph: Option<GraphHead>,
) -> EngineResult<Option<Publication>> {
    let Some(storage) = &manifest.storage_generation else {
        return Ok(None);
    };
    let state = TextState::read(&manifest.storage_home())?;
    if state.as_ref().is_some_and(|s| s.state != TextPhase::Stable) {
        return Err(EngineError::recovery_required(
            "text mutation must be reconciled before publication",
        ));
    }
    let revision = state.as_ref().map_or(0, |s| s.text_revision);
    let digest = state.as_ref().map_or_else(
        || graph_state::fingerprint(&Vec::<String>::new()),
        |s| Ok(s.catalog_digest.clone()),
    )?;
    let mut configuration = serde_json::to_value(manifest)
        .map_err(|_| EngineError::internal("publication configuration encoding failed"))?;
    configuration
        .as_object_mut()
        .ok_or_else(|| EngineError::internal("invalid publication configuration"))?
        .retain(|name, _| {
            matches!(
                name.as_str(),
                "scan"
                    | "indexPolicy"
                    | "defaultEmbeddingModel"
                    | "embeddingRoutes"
                    | "embeddingRuntimes"
                    | "indexVersion"
            )
        });
    let configuration_digest = graph_state::fingerprint(&configuration)?;
    let policy = super::graph_policy::GraphPolicy::read(&manifest.path)?;
    let graph_ready = graph.as_ref().is_some_and(|head| {
        head.text_revision == revision
            && head.catalog_digest == digest
            && policy.as_ref().is_some_and(|p| {
                p.configuration.scope.enabled() && p.policy_fingerprint == head.policy_fingerprint
            })
            && {
                #[cfg(feature = "knowledge-graph-graphdb")]
                {
                    zg_knowledge::graph::schema::build_fingerprint()
                        .is_ok_and(|fp| fp == head.schema_fingerprint)
                }
                #[cfg(not(feature = "knowledge-graph-graphdb"))]
                {
                    false
                }
            }
    });
    if let Some(old) = &manifest.publication
        && old.storage_generation == *storage
        && old.text_revision == revision
        && old.catalog_digest == digest
        && old.configuration_digest == configuration_digest
        && old.graph_ready == graph_ready
        && graph_state::fingerprint(&old.graph)? == graph_state::fingerprint(&graph)?
    {
        return Ok(Some(old.clone()));
    }
    let generation = allocate_generation(manifest.publication.as_ref(), graph.as_ref());
    let knowledge_base_id = manifest
        .publication
        .as_ref()
        .map(|p| p.knowledge_base_id.clone())
        .or_else(|| {
            super::manifest::read_workspace_manifest(&manifest.path)
                .ok()
                .flatten()
                .and_then(|m| m.publication.map(|p| p.knowledge_base_id))
        })
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    Ok(Some(Publication {
        knowledge_base_id,
        generation,
        storage_generation: storage.clone(),
        text_revision: revision,
        catalog_digest: digest,
        configuration_digest,
        graph,
        graph_ready,
    }))
}

fn allocate_generation(previous: Option<&Publication>, graph: Option<&GraphHead>) -> String {
    graph
        .filter(|head| {
            previous
                .and_then(|p| p.graph.as_ref())
                .is_none_or(|old| old.graph_build_id != head.graph_build_id)
        })
        .and_then(|head| head.generation.clone())
        .filter(|id| graph_state::valid_uuid(id) && previous.is_none_or(|p| p.generation != *id))
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string())
}

#[cfg(test)]
mod tests {
    use super::{GraphHead, Publication, allocate_generation};
    #[test]
    fn carrying_an_old_graph_never_reuses_its_publication_generation() {
        let head = GraphHead {
            generation: Some(uuid::Uuid::new_v4().to_string()),
            format_version: 1,
            graph_build_id: uuid::Uuid::new_v4().to_string(),
            text_revision: 1,
            catalog_digest: "a".repeat(64),
            contract_version: zg_knowledge::relations::FACT_CONTRACT_VERSION.into(),
            schema_fingerprint: "b".repeat(64),
            policy_fingerprint: "c".repeat(64),
            domains: std::collections::BTreeMap::default(),
        };
        let old = Publication {
            generation: uuid::Uuid::new_v4().to_string(),
            knowledge_base_id: uuid::Uuid::new_v4().to_string(),
            storage_generation: uuid::Uuid::new_v4().to_string(),
            text_revision: 2,
            catalog_digest: "d".repeat(64),
            configuration_digest: "e".repeat(64),
            graph: Some(head.clone()),
            graph_ready: false,
        };
        let allocated = allocate_generation(Some(&old), Some(&head));
        assert_ne!(Some(allocated.as_str()), head.generation.as_deref());
        assert_ne!(allocated, old.generation);
        let mut candidate = head.clone();
        candidate.graph_build_id = uuid::Uuid::new_v4().to_string();
        candidate.generation = Some(uuid::Uuid::new_v4().to_string());
        assert_eq!(
            Some(allocate_generation(Some(&old), Some(&candidate))),
            candidate.generation
        );
    }
}
