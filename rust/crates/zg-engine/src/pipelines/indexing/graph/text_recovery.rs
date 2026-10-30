//! Explicit text-baseline repair. Never infer a completed text write from graph
//! diagnostics, and never revive the old graph after repairing the text fence.
use super::{
    EngineError, EngineResult, GraphHead, IndexStore, TextState, WorkspaceIndexStorageOptions,
    WorkspaceManifest, check_context, context, graph_state,
};
use crate::api::index::graph::{TextStateRecoveryOptions, TextStateRecoveryResult};
use crate::storage::graph_state::TextPhase;
use std::sync::Arc;

#[expect(
    clippy::too_many_lines,
    reason = "keep snapshot validation, compare-and-set and durable repair confirmation together"
)]
pub(crate) async fn run(
    manifest: WorkspaceManifest,
    options: TextStateRecoveryOptions,
    scanner: &zg_host_native::NativeScanner,
    admission: Arc<crate::workspace::lock::FileLock>,
) -> EngineResult<TextStateRecoveryResult> {
    if manifest.storage_generation.as_deref() != Some(options.generation.as_str()) {
        return Err(EngineError::resource_busy(
            "selected text generation changed",
        ));
    }
    let home = manifest.storage_home();
    let before = TextState::read(&home)?
        .ok_or_else(|| EngineError::invalid_argument("no text state to recover"))?;
    let storage = IndexStore::open(WorkspaceIndexStorageOptions::ReadOnly {
        storage_path: home.clone(),
    })?;
    let snapshot = super::super::pipeline::graph_snapshot(
        &manifest.workspace,
        &storage,
        scanner,
        options.signal.clone(),
        &std::collections::HashSet::default(),
    )
    .await;
    let inventory = storage.list_files();
    let close = storage.close();
    drop(storage);
    close?;
    let sources_match = snapshot?.is_some();
    let digest = graph_state::catalog_digest(&inventory?);
    super::control::run(
        admission,
        context(options.signal.clone()),
        move |operation| {
            check_context(&operation.context)?;
            if crate::workspace::manifest::read_workspace_manifest(&manifest.path)?
                .and_then(|m| m.storage_generation)
                != Some(options.generation.clone())
                || TextState::read(&home)?.as_ref() != Some(&before)
            {
                return Err(EngineError::resource_busy("text recovery base changed"));
            }
            let mut current = before;
            let mut repaired = false;
            if options.repair {
                if options.expected_operation_id.as_ref() != Some(&current.operation_id)
                    || options.expected_attempt != Some(current.attempt)
                    || options.expected_text_revision != Some(current.text_revision)
                {
                    return Err(EngineError::resource_busy("text recovery identity changed"));
                }
                if current.state != TextPhase::Mutating || !sources_match {
                    return Err(EngineError::resource_busy(
                        "text recovery requires a dirty, fully indexed current source set",
                    ));
                }
                let revision = current
                    .text_revision
                    .checked_add(1)
                    .ok_or_else(|| EngineError::storage_failure("text revision overflow"))?;
                // Validate before touching the pointer; unknown HEAD formats fail closed.
                GraphHead::read(&home)?;
                check_context(&operation.context)?;
                GraphHead::revoke(&home)?;
                current.state = TextPhase::Stable;
                current.text_revision = revision;
                current.catalog_digest.clone_from(&digest);
                current.operation_id = uuid::Uuid::new_v4().to_string();
                current.attempt = 1;
                current.updated_epoch_ms = graph_state::now();
                graph_state::write_record(&home.join("graph/STATE.json"), &current).map_err(
                    |_| {
                        EngineError::storage_failure(
                            "text recovery write requires recovery_required",
                        )
                    },
                )?;
                if TextState::read(&home)
                    .map_err(|_| {
                        EngineError::storage_failure(
                            "text recovery readback requires recovery_required",
                        )
                    })?
                    .as_ref()
                    != Some(&current)
                {
                    return Err(EngineError::storage_failure(
                        "text recovery confirmation requires recovery_required",
                    ));
                }
                // Do not overwrite a known repair with a late cancellation result.
                repaired = true;
            }
            Ok(TextStateRecoveryResult {
                generation: options.generation,
                operation_id: current.operation_id,
                attempt: current.attempt,
                text_revision: current.text_revision,
                catalog_digest: current.catalog_digest,
                dirty: current.state == TextPhase::Mutating,
                sources_match,
                repaired,
            })
        },
    )
    .await
}
