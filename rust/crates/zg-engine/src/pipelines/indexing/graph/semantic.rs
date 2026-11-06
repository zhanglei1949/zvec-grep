//! Explicit semantic writes share normal candidate admission, import and selection.
use super::{
    Arc, BuildRecord, Candidate, EngineError, EngineResult, FactCatalog, GraphDbStore,
    GraphIndexResult, GraphPolicy, GraphWorkMode, ProcessingContext, WorkspaceManifest,
    bound_matches, context, control, fingerprint, graph_error, graph_state, read_record,
};
use crate::{
    api::index::graph::SemanticBuildOptions,
    storage::graph_state::{SemanticBuildInput, discard::DiscardRecord},
};
use zg_knowledge::semantic::SemanticOperation;

pub(super) fn apply(
    record: &BuildRecord,
    base: Option<&FactCatalog>,
    catalog: FactCatalog,
    ctx: &ProcessingContext,
) -> EngineResult<FactCatalog> {
    let Some(input) = &record.semantic else {
        return Ok(catalog);
    };
    let base = base.ok_or_else(|| EngineError::resource_busy("semantic base is unavailable"))?;
    let digest =
        zg_knowledge::processing::stable_digest(base).map_err(crate::graph_facts::map_error)?;
    if digest != input.base_facts_digest {
        return Err(EngineError::resource_busy("semantic base facts changed"));
    }
    let mut catalog = catalog
        .apply_sources_at(&input.input, ctx, record.created_epoch_ms)
        .map_err(crate::graph_facts::map_error)?;
    catalog.semantic.last_operation = record.semantic_operation()?;
    Ok(catalog)
}

pub(crate) async fn run(
    manifest: WorkspaceManifest,
    options: SemanticBuildOptions,
    admission: Arc<crate::workspace::lock::FileLock>,
) -> EngineResult<GraphIndexResult> {
    let policy = GraphPolicy::read(&manifest.path)?
        .filter(|p| p.configuration.scope.enabled())
        .ok_or_else(|| {
            EngineError::invalid_argument("semantic build requires an enabled graph policy")
        })?;
    control::run(
        admission,
        context(options.signal.clone()),
        move |operation| build(&manifest, &policy, &options, operation),
    )
    .await
}
fn build(
    manifest: &WorkspaceManifest,
    policy: &GraphPolicy,
    options: &SemanticBuildOptions,
    operation: control::Operation,
) -> EngineResult<GraphIndexResult> {
    let candidate = Candidate::open(manifest, policy, operation, &options.operation_id)?;
    let path = candidate
        .home
        .join("graph/builds")
        .join(&options.operation_id);
    if DiscardRecord::read(&candidate.home, &options.operation_id)?.is_some() {
        return Err(EngineError::resource_busy(
            "semantic attempt was discarded; use a new operation",
        ));
    }
    if let Some(record) = read_record::<BuildRecord>(&path.join("build.json"))? {
        return replay(&candidate, &record, options);
    }
    if path.exists() {
        return Err(EngineError::recovery_required(
            "semantic candidate registration is incomplete",
        ));
    }
    if candidate
        .old_head
        .as_ref()
        .map(|h| h.graph_build_id.as_str())
        != Some(options.expected_graph_build_id.as_str())
    {
        return Err(EngineError::resource_busy(
            "semantic selection base changed",
        ));
    }
    let head = candidate
        .old_head
        .as_ref()
        .ok_or_else(|| EngineError::resource_busy("missing semantic base"))?;
    if candidate.state.state != graph_state::TextPhase::Stable
        || candidate.state.catalog_digest != head.catalog_digest
        || head.policy_fingerprint != policy.policy_fingerprint
    {
        return Err(EngineError::resource_busy(
            "semantic text or policy is not current",
        ));
    }
    if !candidate.configuration_matches()? {
        return Err(EngineError::resource_busy(
            "semantic build requires current code configuration",
        ));
    }
    let base = candidate
        .previous()?
        .ok_or_else(|| EngineError::resource_busy("semantic build requires selected facts"))?;
    if candidate.state.text_revision
        != candidate
            .old_head
            .as_ref()
            .ok_or_else(|| EngineError::resource_busy("missing semantic base"))?
            .text_revision
        || !bound_matches(&candidate.storage, &candidate.files, &base)?
        || base.selection_fingerprint != fingerprint(&manifest.workspace.scan)?
    {
        return Err(EngineError::resource_busy(
            "semantic text or selection changed",
        ));
    }
    let mut record = candidate.record()?;
    record.operation_id.clone_from(&options.operation_id);
    record.semantic = Some(SemanticBuildInput {
        base_facts_digest: zg_knowledge::processing::stable_digest(&base)
            .map_err(crate::graph_facts::map_error)?,
        input: options.input.clone(),
    });
    record.rules_fingerprint = record.expected_rules_fingerprint()?;
    let catalog = apply(&record, Some(&base), base.clone(), &candidate.ctx)?;
    candidate.resources.check(true)?;
    candidate.commit(&record, &catalog)?;
    let mut result = candidate.select(&record, &catalog, GraphWorkMode::GraphOnly)?;
    result.mode = "semantic".into();
    Ok(result)
}
fn replay(
    candidate: &Candidate<'_>,
    record: &BuildRecord,
    options: &SemanticBuildOptions,
) -> EngineResult<GraphIndexResult> {
    crate::graph_facts::module_roots::configuration::validate_inputs(
        &candidate.manifest.workspace.root,
        &record.configuration_inputs,
    )?;
    let expected = record
        .semantic_operation()?
        .ok_or_else(|| EngineError::resource_busy("candidate is not a semantic attempt"))?;
    let requested = SemanticOperation {
        operation_id: options.operation_id.clone(),
        attempt: 1,
        base_graph_id: options.expected_graph_build_id.clone(),
        base_facts_digest: expected.base_facts_digest.clone(),
        input_digest: fingerprint(&options.input)?,
    };
    if record.graph_build_id != options.operation_id
        || record.operation_id != options.operation_id
        || expected != requested
        || record.rules_fingerprint != record.expected_rules_fingerprint()?
    {
        return Err(EngineError::resource_busy(
            "semantic attempt input conflicts",
        ));
    }
    if candidate
        .old_head
        .as_ref()
        .map(|h| h.graph_build_id.as_str())
        != Some(options.operation_id.as_str())
    {
        return Err(EngineError::recovery_required(
            "semantic attempt exists; inspect and explicitly resume or select it",
        ));
    }
    if candidate.state.state != graph_state::TextPhase::Stable
        || candidate.state.text_revision != record.base_text_revision
        || candidate.state.catalog_digest != record.base_catalog_digest
        || candidate.policy.policy_fingerprint != record.policy_fingerprint
    {
        return Err(EngineError::resource_busy(
            "semantic selection text or policy changed",
        ));
    }
    let graph = GraphDbStore::open(
        candidate
            .home
            .join("graph/builds")
            .join(&record.graph_build_id)
            .join("db"),
        true,
        candidate.limits.clone(),
    )
    .map_err(|e| graph_error(&e))?;
    let catalog = graph
        .read_build_facts(&record.spec(), &candidate.ctx)
        .map_err(|e| graph_error(&e))?
        .ok_or_else(|| EngineError::recovery_required("semantic selection has no commit proof"))?;
    if catalog.semantic.last_operation.as_ref() != Some(&expected)
        || !bound_matches(&candidate.storage, &candidate.files, &catalog)?
        || catalog.selection_fingerprint != fingerprint(&candidate.manifest.workspace.scan)?
    {
        return Err(EngineError::recovery_required(
            "semantic selection no longer matches its receipt",
        ));
    }
    Ok(GraphIndexResult {
        build_report: None,
        text_published: false,
        graph_candidate_committed: true,
        graph_selected: true,
        graph_build_id: Some(record.graph_build_id.clone()),
        mode: "semantic_replayed".into(),
        domains: candidate.domains(&catalog)?,
        diagnostics: Vec::new(),
    })
}
