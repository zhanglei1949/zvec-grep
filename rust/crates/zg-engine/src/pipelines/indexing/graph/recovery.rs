//! No implicit recovery on open. Every mutation is admitted under the same
//! writer lock, after all previous native owners have been released.
use super::{
    Arc, BuildRecord, Candidate, EngineError, EngineResult, GraphDbStore, GraphHead, GraphPolicy,
    GraphWorkMode, IndexStore, PathBuf, TextState, WorkspaceIndexStorageOptions, WorkspaceManifest,
    attempt, bound_matches, check_context, committed_recovery, confirmation_diagnostics, context,
    fingerprint, graph_state, read_record, rules_fingerprint,
};
use crate::api::index::graph::{GraphRecoveryAction, GraphRecoveryOptions, GraphRecoveryResult};
use crate::storage::graph_state::TextPhase;
use crate::storage::graph_state::discard::DiscardRecord;
use std::path::Path;
use zg_knowledge::graph::GraphErrorKind;

pub(crate) async fn run(
    home: PathBuf,
    active: Option<WorkspaceManifest>,
    options: GraphRecoveryOptions,
    scanner: &zg_host_native::NativeScanner,
    admission: Arc<crate::workspace::lock::FileLock>,
) -> EngineResult<GraphRecoveryResult> {
    let generation = options
        .generation
        .clone()
        .or_else(|| active.as_ref().and_then(|m| m.storage_generation.clone()))
        .ok_or_else(|| {
            EngineError::invalid_argument("graph recovery requires an explicit generation")
        })?;
    let current = active
        .as_ref()
        .filter(|m| m.storage_generation.as_deref() == Some(generation.as_str()));
    let has_discard =
        DiscardRecord::read(&storage_path(&home, &generation)?, &options.graph_build_id)?.is_some();
    let snapshot_inputs = if let Some(manifest) =
        current.filter(|_| options.action != GraphRecoveryAction::Discard && !has_discard)
    {
        let storage = IndexStore::open(WorkspaceIndexStorageOptions::ReadOnly {
            storage_path: manifest.storage_home(),
        })?;
        let snapshot = super::super::pipeline::graph_snapshot(
            &manifest.workspace,
            &storage,
            scanner,
            options.signal.clone(),
            &std::collections::HashSet::default(),
        )
        .await;
        let close = storage.close();
        drop(storage);
        close?;
        snapshot?
    } else {
        None
    };
    let sources_match = snapshot_inputs.is_some();
    super::control::run(
        admission,
        context(options.signal.clone()),
        move |operation| {
            if options.action == GraphRecoveryAction::Resume {
                resume_candidate(
                    &home,
                    active.as_ref(),
                    &generation,
                    &options,
                    sources_match,
                    snapshot_inputs,
                    &operation,
                )
            } else {
                reconcile(
                    &home,
                    active.as_ref(),
                    &generation,
                    &options,
                    sources_match,
                    operation,
                )
            }
        },
    )
    .await
}
fn evidence_error(error: &zg_knowledge::graph::GraphError) -> EngineError {
    if matches!(
        error.kind,
        GraphErrorKind::Cancelled | GraphErrorKind::Deadline | GraphErrorKind::ResourceExhausted
    ) {
        super::graph_error(error)
    } else {
        invalid()
    }
}
fn invalid() -> EngineError {
    EngineError::storage_failure("graph recovery_required: candidate evidence is inconsistent")
}
fn storage_path(home: &Path, generation: &str) -> EngineResult<PathBuf> {
    let mut path = home.to_path_buf();
    for component in ["generations", generation, "graph"] {
        path.push(component);
        let metadata = std::fs::symlink_metadata(&path).map_err(|_| invalid())?;
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            return Err(invalid());
        }
    }
    Ok(path.parent().ok_or_else(invalid)?.to_path_buf())
}
fn known_path(home: &Path, generation: &str, id: &str) -> EngineResult<PathBuf> {
    let mut path = storage_path(home, generation)?;
    for part in ["graph", "builds", id] {
        path.push(part);
        let metadata = std::fs::symlink_metadata(&path).map_err(|_| invalid())?;
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            return Err(invalid());
        }
    }
    // Pre-commit attempts can stop before creating a database or during DDL.
    match std::fs::symlink_metadata(path.join("db")) {
        Ok(m) if m.is_dir() && !m.file_type().is_symlink() => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        _ => return Err(invalid()),
    }
    Ok(path)
}
fn discarded_result(record: DiscardRecord, generation: &str) -> GraphRecoveryResult {
    let discarded = record.is_removed();
    GraphRecoveryResult {
        graph_build_id: record.graph_build_id,
        generation: generation.into(),
        operation_id: record.operation_id,
        attempt: record.attempt,
        facts_ready: false,
        candidate_committed: record.candidate_committed,
        selected: false,
        eligible: false,
        discarded,
        completed_batches: None,
        total_batches: None,
        diagnostics: vec![
            if discarded {
                "candidate_discarded"
            } else {
                "discard_pending_recovery_required"
            }
            .into(),
        ],
    }
}
fn known_precommit(diagnostic: Option<&serde_json::Value>) -> bool {
    diagnostic.is_some_and(|d| {
        matches!(
            d["stage"].as_str(),
            Some("registered" | "validated" | "failed")
        ) && d["commitEffect"] == "not_applied"
            && d["selectionEffect"] == "not_selected"
    })
}
fn validate_record(record: &BuildRecord, id: &str) -> EngineResult<()> {
    if record.graph_build_id != id
        || !graph_state::valid_uuid(&record.operation_id)
        || record.attempt == 0
        || record.schema_fingerprint
            != zg_knowledge::graph::schema::build_fingerprint()
                .map_err(crate::graph_facts::map_error)?
        || fingerprint(&record.configuration)? != record.policy_fingerprint
        || ![
            &record.base_catalog_digest,
            &record.rules_fingerprint,
            &record.module_roots_fingerprint,
        ]
        .iter()
        .all(|s| graph_state::valid_digest(s))
        || record
            .base_head
            .as_ref()
            .is_some_and(|s| !graph_state::valid_uuid(s))
    {
        return Err(invalid());
    }
    Ok(())
}
#[expect(
    clippy::too_many_lines,
    reason = "keep authoritative proof, selection and diagnostic effects in order"
)]
fn reconcile(
    home: &Path,
    active: Option<&WorkspaceManifest>,
    generation: &str,
    options: &GraphRecoveryOptions,
    sources_match: bool,
    operation: super::control::Operation,
) -> EngineResult<GraphRecoveryResult> {
    let ctx = operation.context.clone();
    check_context(&ctx)?;
    let storage = storage_path(home, generation)?;
    if let Some(discard) = DiscardRecord::read(&storage, &options.graph_build_id)? {
        if GraphHead::read(&storage)?
            .is_some_and(|head| head.graph_build_id == discard.graph_build_id)
        {
            return Err(invalid());
        }
        if options.action == GraphRecoveryAction::Inspect {
            if options
                .expected_operation_id
                .as_ref()
                .is_some_and(|id| id != &discard.operation_id)
                || options
                    .expected_attempt
                    .is_some_and(|a| a != discard.attempt)
            {
                return Err(EngineError::resource_busy("graph discard identity changed"));
            }
            return Ok(discarded_result(discard, generation));
        }
        if options.action != GraphRecoveryAction::Discard {
            return Err(EngineError::storage_failure(
                "candidate discard recovery_required",
            ));
        }
        if options.expected_operation_id.as_ref() != Some(&discard.operation_id)
            || options.expected_attempt != Some(discard.attempt)
        {
            return Err(EngineError::resource_busy("graph discard identity changed"));
        }
        return Ok(discarded_result(discard.finish(&storage)?, generation));
    }
    let path = known_path(home, generation, &options.graph_build_id)?;
    let record: BuildRecord = read_record(&path.join("build.json"))?.ok_or_else(invalid)?;
    validate_record(&record, &options.graph_build_id)?;
    if options
        .expected_operation_id
        .as_ref()
        .is_some_and(|s| s != &record.operation_id)
        || options
            .expected_attempt
            .is_some_and(|a| a != record.attempt)
    {
        return Err(EngineError::resource_busy("graph recovery attempt changed"));
    }
    let diagnostic = read_record::<serde_json::Value>(&path.join("attempt.json"))?;
    if let Some(diagnostic) = &diagnostic
        && (diagnostic["operationId"] != record.operation_id
            || diagnostic["attempt"] != record.attempt)
    {
        return Err(invalid());
    }
    let storage_home = path
        .parent()
        .and_then(Path::parent)
        .and_then(Path::parent)
        .ok_or_else(invalid)?;
    let head = GraphHead::read(storage_home)?;
    let generation_selected =
        active.is_some_and(|m| m.storage_generation.as_deref() == Some(generation));
    let pointer = head
        .as_ref()
        .filter(|h| h.graph_build_id == record.graph_build_id);
    let selected = generation_selected && pointer.is_some();
    if options.action == GraphRecoveryAction::Discard && pointer.is_some() {
        return Err(EngineError::resource_busy(
            "cannot discard a graph referenced by HEAD",
        ));
    }
    if options.action == GraphRecoveryAction::Discard && known_precommit(diagnostic.as_ref()) {
        // The durable protocol must enter commit_pending before calling native
        // root commit. These known pre-root attempts can have incomplete DDL;
        // this is protocol evidence, not a fabricated native readback.
        check_context(&ctx)?;
        let discard = DiscardRecord::start(
            storage_home,
            &record.graph_build_id,
            &record.operation_id,
            record.attempt,
            false,
        )?;
        let mut result = discarded_result(discard.finish(storage_home)?, generation);
        result
            .diagnostics
            .push("known_precommit_protocol_evidence".into());
        return Ok(result);
    }
    let readonly = options.action == GraphRecoveryAction::Inspect;
    let opened = if record.import_budget.is_some() {
        GraphDbStore::open_import(path.join("db"), readonly, operation.limits(), &ctx)
    } else {
        GraphDbStore::open(path.join("db"), readonly, operation.limits())
    };
    let graph = match opened {
        Ok(graph) => graph,
        Err(error)
            if readonly
                && pointer.is_none()
                && known_precommit(diagnostic.as_ref())
                && !matches!(
                    error.kind,
                    GraphErrorKind::Cancelled
                        | GraphErrorKind::Deadline
                        | GraphErrorKind::ResourceExhausted
                ) =>
        {
            return Ok(GraphRecoveryResult {
                graph_build_id: record.graph_build_id,
                generation: generation.into(),
                operation_id: record.operation_id,
                attempt: record.attempt,
                facts_ready: false,
                candidate_committed: false,
                selected: false,
                eligible: false,
                discarded: false,
                completed_batches: None,
                total_batches: None,
                diagnostics: vec![
                    "known_precommit_protocol_evidence".into(),
                    "native_candidate_not_inspected".into(),
                ],
            });
        }
        Err(error) => return Err(evidence_error(&error)),
    };
    let progress = if record.import_budget.is_some() {
        Some(graph.inspect_import(&ctx).map_err(|e| evidence_error(&e))?)
    } else {
        None
    };
    if progress
        .as_ref()
        .is_some_and(|p| Some(&p.budget) != record.import_budget.as_ref())
    {
        return Err(invalid());
    }
    let facts_ready = progress.as_ref().is_none_or(|p| p.finalized);
    let catalog = if facts_ready {
        Some(graph.read_facts(&ctx).map_err(|e| evidence_error(&e))?)
    } else {
        None
    };
    let proof = if facts_ready {
        graph
            .inspect_build(&record.spec(), &ctx)
            .map_err(|e| evidence_error(&e))?
    } else {
        graph
            .ensure_no_build_commit()
            .map_err(|e| evidence_error(&e))?;
        None
    };
    if pointer.is_some_and(|h| {
        h.text_revision != record.base_text_revision
            || h.catalog_digest != record.base_catalog_digest
            || h.policy_fingerprint != record.policy_fingerprint
            || h.schema_fingerprint != record.schema_fingerprint
    }) || (pointer.is_some() && proof.is_none())
    {
        return Err(invalid());
    }
    let mut result = GraphRecoveryResult {
        graph_build_id: record.graph_build_id.clone(),
        generation: generation.into(),
        operation_id: record.operation_id.clone(),
        attempt: record.attempt,
        facts_ready,
        candidate_committed: proof.is_some(),
        selected,
        eligible: false,
        discarded: false,
        completed_batches: progress.as_ref().map(|p| p.completed_batches),
        total_batches: progress.as_ref().map(|p| p.total_batches),
        diagnostics: Vec::new(),
    };
    if !generation_selected {
        result
            .diagnostics
            .push("text_generation_not_selected".into());
    }
    if !sources_match {
        result.diagnostics.push("sources_not_current".into());
    }
    if !facts_ready {
        result
            .diagnostics
            .push("import_incomplete_retry_requires_new_build".into());
    }
    let policy = GraphPolicy::read(home)?;
    let state = TextState::read(storage_home)?;
    let policy_matches = policy.as_ref().is_some_and(|p| {
        p.policy_fingerprint == record.policy_fingerprint && p.configuration.scope.enabled()
    });
    let state_matches = state.as_ref().is_some_and(|s| {
        s.state == TextPhase::Stable
            && s.text_revision == record.base_text_revision
            && s.catalog_digest == record.base_catalog_digest
    });
    if let (Some(manifest), Some(catalog)) =
        (active.filter(|_| generation_selected), catalog.as_ref())
    {
        let text = IndexStore::open(WorkspaceIndexStorageOptions::ReadOnly {
            storage_path: manifest.storage_home(),
        })?;
        let files = text.list_files()?;
        let valid = graph_state::catalog_digest(&files) == record.base_catalog_digest
            && bound_matches(&text, &files, catalog)?
            && catalog.selection_fingerprint == fingerprint(&manifest.workspace.scan)?
            && catalog
                .files
                .iter()
                .all(|f| f.record.storage_generation == generation);
        text.close()?;
        result.eligible = proof.is_some()
            && state_matches
            && policy_matches
            && sources_match
            && valid
            && (selected || head.as_ref().map(|h| &h.graph_build_id) == record.base_head.as_ref())
            && record.rules_fingerprint == rules_fingerprint()?;
    }
    // Read-only inspection never repairs a dirty fence, checkpoints or writes
    // attempt metadata. A writable reopen independently verifies durable proof.
    if !readonly {
        graph.checkpoint().map_err(|e| evidence_error(&e))?;
    }
    drop(graph);
    if options.action == GraphRecoveryAction::Select && !selected {
        if !result.eligible {
            return Err(EngineError::resource_busy(
                "graph recovery base/source/policy changed; rebuild required",
            ));
        }
        let manifest = active.ok_or_else(invalid)?;
        let policy = policy.as_ref().ok_or_else(invalid)?;
        if head.as_ref().map(|h| &h.graph_build_id) != record.base_head.as_ref() {
            return Err(EngineError::resource_busy(
                "graph recovery selection base changed",
            ));
        }
        check_context(&ctx)?;
        let candidate = Candidate::open(manifest, policy, operation, &record.graph_build_id)?;
        let selected = candidate.select(
            &record,
            catalog.as_ref().ok_or_else(invalid)?,
            GraphWorkMode::GraphOnly,
        )?;
        result.selected = selected.graph_selected;
        return Ok(result);
    }
    if !readonly {
        check_context(&ctx)?;
        // A candidate-generation HEAD is only prepared, not an active selection.
        // Confirm that neither pointer changed while native owners were closing.
        if GraphHead::read(storage_home)? != head
            || crate::workspace::manifest::read_workspace_manifest(home)?
                .as_ref()
                .and_then(|m| m.storage_generation.as_ref())
                != active.and_then(|m| m.storage_generation.as_ref())
        {
            return Err(invalid());
        }
        attempt(
            &path,
            &record,
            if selected {
                "selected"
            } else if proof.is_some() {
                "confirmed"
            } else {
                "failed"
            },
            if proof.is_some() {
                "committed"
            } else {
                "not_applied"
            },
            if selected { "selected" } else { "not_selected" },
            ctx.cancellation.is_cancelled(),
            Some("explicitly_reconciled"),
        )?;
        if options.action == GraphRecoveryAction::Discard {
            // Admission has drained every text/graph residency lease and this
            // operation released its own native database above. Unknown proof
            // must be reconciled before reaching this point.
            let discard = DiscardRecord::start(
                storage_home,
                &record.graph_build_id,
                &record.operation_id,
                record.attempt,
                proof.is_some(),
            )?;
            return Ok(discarded_result(discard.finish(storage_home)?, generation));
        }
    }
    Ok(result)
}

/// Explicit host-driven replay of an interrupted pre-commit import. Reconstructs
/// the fixed-input catalog from the frozen sources and continues the persisted
/// batch prefix in place; never replays a committed, selected or unknown
/// candidate, and refuses source/state/policy/base drift instead of guessing.
#[expect(
    clippy::too_many_lines,
    reason = "keep identity, drift guards, resume and authoritative re-inspection in order"
)]
fn resume_candidate(
    home: &Path,
    active: Option<&WorkspaceManifest>,
    generation: &str,
    options: &GraphRecoveryOptions,
    sources_match: bool,
    inputs: Option<std::collections::HashMap<PathBuf, Vec<u8>>>,
    operation: &super::control::Operation,
) -> EngineResult<GraphRecoveryResult> {
    let ctx = operation.context.clone();
    let limits = operation.limits();
    check_context(&ctx)?;
    let storage = storage_path(home, generation)?;
    if DiscardRecord::read(&storage, &options.graph_build_id)?.is_some() {
        return Err(EngineError::storage_failure(
            "candidate discard recovery_required",
        ));
    }
    let path = known_path(home, generation, &options.graph_build_id)?;
    let record: BuildRecord = read_record(&path.join("build.json"))?.ok_or_else(invalid)?;
    validate_record(&record, &options.graph_build_id)?;
    if options.expected_operation_id.as_ref() != Some(&record.operation_id)
        || options.expected_attempt != Some(record.attempt)
    {
        return Err(EngineError::resource_busy("graph recovery attempt changed"));
    }
    let diagnostic = read_record::<serde_json::Value>(&path.join("attempt.json"))?;
    if let Some(diagnostic) = &diagnostic
        && (diagnostic["operationId"] != record.operation_id
            || diagnostic["attempt"] != record.attempt)
    {
        return Err(invalid());
    }
    // attempt.json is a host diagnostic, not authoritative: the native database
    // state decides resumability and is classified below after the drift guards.
    let storage_home = path
        .parent()
        .and_then(Path::parent)
        .and_then(Path::parent)
        .ok_or_else(invalid)?;
    let head = GraphHead::read(storage_home)?;
    if head
        .as_ref()
        .is_some_and(|h| h.graph_build_id == record.graph_build_id)
    {
        return Err(EngineError::resource_busy(
            "graph resume candidate already selected",
        ));
    }
    if head.as_ref().map(|h| &h.graph_build_id) != record.base_head.as_ref() {
        return Err(EngineError::resource_busy(
            "graph resume base changed; rebuild required",
        ));
    }
    let state = TextState::read(storage_home)?;
    let state_matches = state.as_ref().is_some_and(|s| {
        s.state == TextPhase::Stable
            && s.text_revision == record.base_text_revision
            && s.catalog_digest == record.base_catalog_digest
    });
    if !sources_match || !state_matches {
        return Err(EngineError::resource_busy(
            "graph resume input changed; rebuild required",
        ));
    }
    let inputs = inputs
        .ok_or_else(|| EngineError::resource_busy("graph resume requires current sources"))?;
    let manifest = active.ok_or_else(invalid)?;
    let policy = GraphPolicy::read(home)?.ok_or_else(invalid)?;
    if policy.policy_fingerprint != record.policy_fingerprint
        || !policy.configuration.scope.enabled()
    {
        return Err(EngineError::resource_busy(
            "graph resume policy changed; rebuild required",
        ));
    }
    // A candidate that stopped before creating its database has no batch prefix
    // to resume; require a new build rather than guessing.
    let db_path = path.join("db");
    if !db_path.is_dir()
        || std::fs::read_dir(&db_path)
            .map_err(|_| invalid())?
            .next()
            .is_none()
    {
        return Err(EngineError::resource_busy(
            "graph resume needs an interrupted import; rebuild required",
        ));
    }
    // Classify from the authoritative native state, not the host diagnostic: a
    // committed candidate is idempotent, a verified pre-commit prefix resumes.
    let pre = GraphDbStore::open_import(db_path, true, limits.clone(), &ctx)
        .map_err(|e| evidence_error(&e))?;
    let pre_progress = pre.inspect_import(&ctx).map_err(|e| evidence_error(&e))?;
    let already_committed = if pre_progress.finalized {
        pre.inspect_build(&record.spec(), &ctx)
            .map_err(|e| evidence_error(&e))?
            .is_some()
    } else {
        pre.ensure_no_build_commit()
            .map_err(|e| evidence_error(&e))?;
        false
    };
    drop(pre);
    if already_committed {
        attempt(
            &path,
            &record,
            "confirmed",
            "committed",
            "not_selected",
            ctx.cancellation.is_cancelled(),
            Some("explicitly_resumed"),
        )?;
        return Ok(GraphRecoveryResult {
            graph_build_id: record.graph_build_id.clone(),
            generation: generation.into(),
            operation_id: record.operation_id.clone(),
            attempt: record.attempt,
            facts_ready: true,
            candidate_committed: true,
            selected: false,
            eligible: false,
            discarded: false,
            completed_batches: Some(pre_progress.completed_batches),
            total_batches: Some(pre_progress.total_batches),
            diagnostics: vec!["resume_already_committed".into()],
        });
    }
    check_context(&ctx)?;
    // Reconstruct the fixed-input catalog from the frozen sources. collect()
    // re-extracts each file and verifies entity ids and source digests against
    // the store, and the adapter re-checks plan_digest, so a drifted source
    // cannot silently resume.
    let candidate = Candidate::open(manifest, &policy, operation.clone(), &record.graph_build_id)?;
    let previous = candidate.previous()?;
    let catalog = candidate.collect(&inputs, Vec::new(), previous.as_ref())?;
    let mut diagnostics = candidate.resume(&record, &catalog)?;
    drop(candidate);
    // The candidate is committed here. Use independent bounded native/context
    // control for the host re-check; late caller stops remain diagnostic only.
    let confirmation = operation.confirmation().map_err(|error| {
        committed_recovery(
            &path,
            &record,
            ctx.cancellation.is_cancelled(),
            error.code(),
        )
    })?;
    let checked = (|| {
        let graph = GraphDbStore::open_import(
            path.join("db"),
            true,
            confirmation.limits(),
            confirmation.context(),
        )
        .map_err(|e| evidence_error(&e))?;
        let progress = graph
            .inspect_import(confirmation.context())
            .map_err(|e| evidence_error(&e))?;
        let proof = graph
            .inspect_build(&record.spec(), confirmation.context())
            .map_err(|e| evidence_error(&e))?;
        drop(graph);
        if !progress.finalized || proof.is_none() {
            return Err(invalid());
        }
        attempt(
            &path,
            &record,
            "confirmed",
            "committed",
            "not_selected",
            ctx.cancellation.is_cancelled(),
            Some("explicitly_resumed"),
        )?;
        Ok(progress)
    })()
    .map_err(|error: EngineError| {
        committed_recovery(
            &path,
            &record,
            ctx.cancellation.is_cancelled(),
            error.code(),
        )
    })?;
    diagnostics.push("import_resumed".into());
    for diagnostic in confirmation_diagnostics(confirmation.original_stop()) {
        if !diagnostics.contains(&diagnostic) {
            diagnostics.push(diagnostic);
        }
    }
    Ok(GraphRecoveryResult {
        graph_build_id: record.graph_build_id,
        generation: generation.into(),
        operation_id: record.operation_id,
        attempt: record.attempt,
        facts_ready: true,
        candidate_committed: true,
        selected: false,
        eligible: false,
        discarded: false,
        completed_batches: Some(checked.completed_batches),
        total_batches: Some(checked.total_batches),
        diagnostics,
    })
}
