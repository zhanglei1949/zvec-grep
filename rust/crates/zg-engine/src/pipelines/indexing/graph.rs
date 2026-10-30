//! Optional graph construction. The service owns admission and the final text
//! manifest; native graph connections/transactions stay in one blocking task.
#[path = "graph/control.rs"]
mod control;
#[path = "graph/coverage.rs"]
mod coverage;
#[path = "graph/recovery.rs"]
pub(super) mod recovery;
#[path = "graph/resources.rs"]
mod resources;
#[path = "graph/text_recovery.rs"]
pub(super) mod text_recovery;
use crate::{
    EngineError, EngineResult,
    api::index::graph::{
        GraphDomainResult, GraphIndexResult, GraphLanguage, GraphOptions, GraphReady, GraphScope,
        PackageLayout,
    },
    domain::{FileFormat, FileRecord},
    extraction::{self, TextSource},
    graph_facts::{
        binding::FactCapture,
        module_roots::{ModuleRoot, ModuleRoots, PackageLayout as KgLayout},
    },
    storage::{
        IndexStore,
        graph_state::{self, GraphHead, TextState, fingerprint, read_record, write_record},
        types::WorkspaceIndexStorageOptions,
    },
    workspace::{graph_policy::GraphPolicy, manifest::WorkspaceManifest},
};
use serde::Serialize;
use std::{
    collections::{BTreeMap, HashMap},
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio_util::sync::CancellationToken;
use zg_knowledge::{
    graph::{
        GraphDbStore, GraphErrorKind, GraphLimits, GraphOutcome, ImportBudget, build::BuildScope,
        profile,
    },
    processing::ProcessingContext,
    relations::{
        Language,
        bound::{
            BoundFileFacts, FactCatalog, FileRecord as BoundRecord, IndexStatus, entity_ids_digest,
        },
    },
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum GraphWorkMode {
    GraphOnly,
    TextUpdate,
    TextRebuild,
}
impl GraphWorkMode {
    fn text_published(self) -> bool {
        self == Self::TextUpdate
    }
    fn label(self) -> &'static str {
        if self == Self::GraphOnly {
            "graph_only"
        } else {
            "text_and_graph"
        }
    }
    fn result_domains(
        self,
        domains: BTreeMap<String, GraphDomainResult>,
    ) -> BTreeMap<String, GraphDomainResult> {
        if self == Self::TextRebuild {
            domains
                .into_iter()
                .map(|(key, mut domain)| {
                    if domain.ready == GraphReady::Ready {
                        domain.ready = GraphReady::NotReady("manifest_not_selected".into());
                    }
                    (key, domain)
                })
                .collect()
        } else {
            domains
        }
    }
}

fn context(signal: Option<CancellationToken>) -> ProcessingContext {
    ProcessingContext {
        max_input_bytes: 256 * 1024 * 1024,
        max_output_items: 30_000_000,
        deadline: Some(Instant::now() + Duration::from_secs(600)),
        cancellation: signal.unwrap_or_default(),
    }
}
fn roots(options: &GraphOptions) -> EngineResult<ModuleRoots> {
    ModuleRoots::new(
        options
            .module_roots
            .iter()
            .map(|r| ModuleRoot {
                root: r.root.clone(),
                language: match r.language {
                    GraphLanguage::Rust => Language::Rust,
                    GraphLanguage::Python => Language::Python,
                },
                namespace: r.namespace.clone(),
                module_prefix: r.module_prefix.clone(),
                layout: match r.layout {
                    PackageLayout::Flat => KgLayout::Flat,
                    PackageLayout::Src => KgLayout::Src,
                    PackageLayout::ModRs => KgLayout::ModRs,
                },
            })
            .collect(),
    )
}

pub(super) fn capture(
    manifest: &WorkspaceManifest,
    storage: &IndexStore,
    policy: &GraphPolicy,
    signal: Option<CancellationToken>,
) -> EngineResult<Arc<FactCapture>> {
    let state = match TextState::read(&manifest.storage_home())? {
        Some(s) => s,
        None => TextState::baseline(
            &manifest.storage_home(),
            graph_state::catalog_digest(&storage.list_files()?),
        )?,
    };
    Ok(FactCapture::new(
        manifest.storage_generation.clone().ok_or_else(|| {
            EngineError::storage_failure("graph construction requires a published text generation")
        })?,
        state.text_revision,
        roots(&policy.configuration)?,
        context(signal),
    ))
}

use crate::storage::graph_state::{BuildRecord, bound_matches, rules_fingerprint};

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct AttemptDiagnostic<'a> {
    format_version: u32,
    operation_id: &'a str,
    attempt: u64,
    stage: &'a str,
    commit_effect: &'a str,
    selection_effect: &'a str,
    cancellation_requested: bool,
    error_code: Option<&'a str>,
}
fn attempt(
    path: &std::path::Path,
    record: &BuildRecord,
    stage: &str,
    committed: &str,
    selection: &str,
    cancelled: bool,
    error: Option<&str>,
) -> EngineResult<()> {
    write_record(
        &path.join("attempt.json"),
        &AttemptDiagnostic {
            format_version: 1,
            operation_id: &record.operation_id,
            attempt: record.attempt,
            stage,
            commit_effect: committed,
            selection_effect: selection,
            cancellation_requested: cancelled,
            error_code: error,
        },
    )
}

fn graph_error(error: &zg_knowledge::graph::GraphError) -> EngineError {
    if error.kind == GraphErrorKind::RecoveryRequired
        || matches!(
            error.outcome,
            Some(GraphOutcome::Unknown | GraphOutcome::InTransaction)
        )
    {
        EngineError::storage_failure("graph candidate recovery_required")
    } else {
        match error.kind {
            GraphErrorKind::Conflict => EngineError::resource_busy("graph candidate conflict"),
            GraphErrorKind::Cancelled => {
                EngineError::cancelled("graph candidate cancelled before selection")
            }
            GraphErrorKind::Deadline => {
                EngineError::deadline_exceeded("graph candidate deadline exceeded")
            }
            GraphErrorKind::ResourceExhausted | GraphErrorKind::InvalidInput => {
                EngineError::invalid_argument("graph candidate input or budget rejected")
            }
            GraphErrorKind::ReadOnly => {
                EngineError::permission_denied("graph candidate is read only")
            }
            _ => EngineError::storage_failure(format!(
                "graph candidate {:?} at {}",
                error.kind, error.stage
            )),
        }
    }
}
fn check_context(context: &ProcessingContext) -> EngineResult<()> {
    context.check().map_err(crate::graph_facts::map_error)
}

fn confirmation_diagnostics(stop: Option<zg_knowledge::graph::QueryStopReason>) -> Vec<String> {
    match stop {
        Some(zg_knowledge::graph::QueryStopReason::Cancelled) => {
            vec!["cancellation_arrived_after_commit".into()]
        }
        Some(zg_knowledge::graph::QueryStopReason::Deadline) => {
            vec!["deadline_arrived_after_commit".into()]
        }
        None => Vec::new(),
    }
}

fn committed_recovery(
    path: &std::path::Path,
    record: &BuildRecord,
    cancelled: bool,
    error: &str,
) -> EngineError {
    let _ = attempt(
        path,
        record,
        "confirmation_required",
        "committed",
        "not_selected",
        cancelled,
        Some(error),
    );
    EngineError::storage_failure("graph candidate committed; confirmation recovery_required")
}

#[cfg(test)]
type AfterResumeCommit = Arc<dyn Fn(&str, &control::Operation) + Send + Sync>;
#[cfg(test)]
static AFTER_RESUME_COMMIT: std::sync::OnceLock<std::sync::RwLock<Option<AfterResumeCommit>>> =
    std::sync::OnceLock::new();

#[cfg(test)]
fn set_after_resume_commit(hook: Option<AfterResumeCommit>) {
    *AFTER_RESUME_COMMIT
        .get_or_init(|| std::sync::RwLock::new(None))
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = hook;
}

#[cfg(test)]
pub(super) fn clear_after_resume_commit() {
    set_after_resume_commit(None);
}

#[cfg(test)]
pub(super) fn stop_after_resume_commit(
    expected_id: String,
    reason: zg_knowledge::graph::QueryStopReason,
) {
    set_after_resume_commit(Some(Arc::new(move |id, operation| {
        if id == expected_id {
            operation.stop_for_test(reason);
        }
    })));
}

#[cfg(test)]
fn after_resume_commit(id: &str, operation: &control::Operation) {
    let hook = AFTER_RESUME_COMMIT
        .get_or_init(|| std::sync::RwLock::new(None))
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    if let Some(hook) = hook {
        hook(id, operation);
    }
}

/// A dropped async caller cannot release admission while native work continues.
async fn in_worker<T: Send + 'static>(
    admission: Arc<crate::workspace::lock::FileLock>,
    action: impl FnOnce() -> EngineResult<T> + Send + 'static,
) -> EngineResult<T> {
    tokio::task::spawn_blocking(move || {
        let _admission = admission;
        action()
    })
    .await
    .map_err(|_| EngineError::internal("graph worker failed"))?
}

pub(super) async fn run(
    manifest: WorkspaceManifest,
    policy: GraphPolicy,
    inputs: HashMap<PathBuf, Vec<u8>>,
    captured: Vec<BoundFileFacts>,
    signal: Option<CancellationToken>,
    work: GraphWorkMode,
    admission: Arc<crate::workspace::lock::FileLock>,
) -> EngineResult<GraphIndexResult> {
    control::run(admission, context(signal), move |operation| {
        let id = uuid::Uuid::new_v4().to_string();
        let outcome = run_blocking(&manifest, &policy, &inputs, captured, operation, work, &id);
        match outcome {
            Ok(value) => Ok(value),
            Err(error) => {
                let build = manifest.storage_home().join("graph/builds").join(&id);
                let attempt: Option<serde_json::Value> = read_record(&build.join("attempt.json"))
                    .map_err(|_| {
                    EngineError::storage_failure("graph build diagnostic recovery_required")
                })?;
                if attempt.as_ref().is_some_and(|a| {
                    a["commitEffect"] == "unknown"
                        || a["selectionEffect"] == "unknown"
                        || a["stage"]
                            .as_str()
                            .is_some_and(|s| s.ends_with("_pending") || s == "unknown")
                }) {
                    return Err(EngineError::storage_failure(
                        "graph build recovery_required",
                    ));
                }
                if error.code() == EngineError::CANCELLED
                    || (work == GraphWorkMode::GraphOnly
                        && matches!(
                            error.code(),
                            EngineError::DEADLINE_EXCEEDED | EngineError::INVALID_ARGUMENT
                        ))
                    || error.message().contains("recovery_required")
                {
                    return Err(error);
                }
                let mut result = GraphIndexResult::unavailable(error.code(), work.text_published());
                if let Some(help) = error.help() {
                    result.diagnostics.push(help.into());
                }
                result.mode = "failed".into();
                result.graph_candidate_committed = attempt
                    .as_ref()
                    .is_some_and(|a| a["commitEffect"] == "committed");
                if attempt.is_some() {
                    result.graph_build_id = Some(id);
                }
                Ok(result)
            }
        }
    })
    .await
}

fn run_blocking(
    manifest: &WorkspaceManifest,
    policy: &GraphPolicy,
    inputs: &HashMap<PathBuf, Vec<u8>>,
    captured: Vec<BoundFileFacts>,
    operation: control::Operation,
    work: GraphWorkMode,
    id: &str,
) -> EngineResult<GraphIndexResult> {
    let _profile = profile::BuildProfile::start("host.graph_build");
    let _stage = profile::stage("build");
    let candidate = profile::measure("host.open", || {
        Candidate::open(manifest, policy, operation, id)
    })?;
    let previous = profile::measure("host.previous", || candidate.previous())?;
    if let (Some(head), Some(facts)) = (&candidate.old_head, &previous)
        && head.text_revision == candidate.state.text_revision
        && head.catalog_digest == candidate.state.catalog_digest
        && facts.selection_fingerprint == fingerprint(&manifest.workspace.scan)?
        && bound_matches(&candidate.storage, &candidate.files, facts)?
    {
        return Ok(GraphIndexResult {
            text_published: work.text_published(),
            graph_candidate_committed: true,
            graph_selected: true,
            graph_build_id: Some(head.graph_build_id.clone()),
            mode: "unchanged".into(),
            domains: candidate.domains(facts)?,
            diagnostics: Vec::new(),
        });
    }
    let catalog = profile::measure("host.collect", || {
        candidate.collect(inputs, captured, previous.as_ref())
    })?;
    candidate.resources.check(true)?;
    let record = candidate.record()?;
    profile::measure("host.commit", || candidate.commit(&record, &catalog))?;
    profile::measure("host.select", || candidate.select(&record, &catalog, work))
}

struct Candidate<'a> {
    resources: resources::CandidateGuard,
    manifest: &'a WorkspaceManifest,
    policy: &'a GraphPolicy,
    id: &'a str,
    ctx: ProcessingContext,
    limits: GraphLimits,
    operation: control::Operation,
    home: PathBuf,
    storage: IndexStore,
    files: Vec<FileRecord>,
    state: TextState,
    old_head: Option<GraphHead>,
    schema: String,
}
impl<'a> Candidate<'a> {
    fn open(
        manifest: &'a WorkspaceManifest,
        policy: &'a GraphPolicy,
        operation: control::Operation,
        id: &'a str,
    ) -> EngineResult<Self> {
        let limits = operation.limits();
        let ctx = operation.context.clone();
        check_context(&ctx)?;
        let home = manifest.storage_home();
        let storage = IndexStore::open(WorkspaceIndexStorageOptions::ReadOnly {
            storage_path: home.clone(),
        })?;
        let files = storage.list_files()?;
        let state = TextState::baseline(&home, graph_state::catalog_digest(&files))?;
        let old_head = GraphHead::read(&home)?;
        let schema = zg_knowledge::graph::schema::build_fingerprint()
            .map_err(crate::graph_facts::map_error)?;
        Ok(Self {
            resources: resources::CandidateGuard::new(manifest, id),
            manifest,
            policy,
            id,
            ctx,
            limits,
            operation,
            home,
            storage,
            files,
            state,
            old_head,
            schema,
        })
    }
    fn previous(&self) -> EngineResult<Option<FactCatalog>> {
        let Some(head) = &self.old_head else {
            return Ok(None);
        };
        if head.schema_fingerprint != self.schema
            || head.policy_fingerprint != self.policy.policy_fingerprint
        {
            return Ok(None);
        }
        let path = self.home.join("graph/builds").join(&head.graph_build_id);
        let Some(record) = read_record::<BuildRecord>(&path.join("build.json"))? else {
            return Err(EngineError::storage_failure(
                "graph HEAD has no immutable build input",
            ));
        };
        if record.rules_fingerprint != rules_fingerprint()? {
            return Ok(None);
        }
        if record.graph_build_id != head.graph_build_id
            || record.base_text_revision != head.text_revision
            || record.base_catalog_digest != head.catalog_digest
            || record.policy_fingerprint != head.policy_fingerprint
            || record.schema_fingerprint != head.schema_fingerprint
        {
            return Err(EngineError::storage_failure(
                "graph HEAD disagrees with immutable build input",
            ));
        }
        let graph = GraphDbStore::open(path.join("db"), true, self.limits.clone())
            .map_err(|e| graph_error(&e))?;
        if graph
            .inspect_build(&record.spec(), &self.ctx)
            .map_err(|e| graph_error(&e))?
            .is_none()
        {
            return Err(EngineError::storage_failure(
                "selected graph has no commit proof",
            ));
        }
        graph
            .read_facts(&self.ctx)
            .map(Some)
            .map_err(|e| graph_error(&e))
    }
    fn collect(
        &self,
        inputs: &HashMap<PathBuf, Vec<u8>>,
        captured: Vec<BoundFileFacts>,
        previous: Option<&FactCatalog>,
    ) -> EngineResult<FactCatalog> {
        let generation = self.manifest.storage_generation.clone().ok_or_else(|| {
            EngineError::storage_failure("graph construction requires a published text generation")
        })?;
        let collector = FactCapture::new(
            generation,
            self.state.text_revision,
            roots(&self.policy.configuration)?,
            self.ctx.clone(),
        );
        let mut captured = captured
            .into_iter()
            .map(|f| (f.record.path.clone(), f))
            .collect::<BTreeMap<_, _>>();
        let mut files = Vec::new();
        for file in &self.files {
            check_context(&self.ctx)?;
            let path = file.relative_path.to_path_buf();
            let Some(name) = path.to_str().filter(|p| {
                !p.contains(['\\', ':', '\0'])
                    && !p.split('/').any(|c| matches!(c, "" | "." | ".."))
            }) else {
                continue;
            };
            if !file.index_status.is_indexed() {
                return Err(EngineError::resource_busy("text contains unfinished files"));
            }
            self.ctx
                .check_output(
                    usize::try_from(file.index_status.entity_count()).unwrap_or(usize::MAX),
                )
                .map_err(crate::graph_facts::map_error)?;
            let ids = profile::measure("host.entity_ids", || self.storage.file_entity_ids(file))?;
            let digest = entity_ids_digest(&ids).map_err(crate::graph_facts::map_error)?;
            let reusable = previous.and_then(|c| {
                c.files.iter().find(|b| {
                    b.record.path == name
                        && b.record.content_hash == file.snapshot.content_hash
                        && b.record.file_id == file.id.get()
                        && b.record.entity_ids_digest == digest
                        && b.record.storage_generation == collector.generation
                })
            });
            let mut bound = if let Some(b) = captured.remove(name).or_else(|| reusable.cloned()) {
                b
            } else {
                profile::measure("host.extract", || {
                    self.extract(file, inputs, &collector, &ids, &digest)
                })?
            };
            if bound.record.content_hash != file.snapshot.content_hash
                || bound.record.entity_ids_digest != digest
                || bound.record.storage_generation != collector.generation
            {
                return Err(EngineError::resource_busy(
                    "captured graph binding no longer matches text",
                ));
            }
            bound.record.index_status = IndexStatus::Indexed;
            bound.record.text_revision = self.state.text_revision;
            self.restrict_domain(&mut bound);
            if bound.facts.is_some()
                && bound.text_binding_digest.as_ref()
                    != Some(&crate::graph_facts::text_binding::stored_digest(
                        &self.storage,
                        &ids,
                    )?)
            {
                return Err(EngineError::resource_busy(
                    "graph text or fragment mapping changed; rebuild the text index explicitly",
                ).with_help("Run `zg index --rebuild` to rebuild text and graph with matching bodies and fragments."));
            }
            files.push(bound);
        }
        let selection = fingerprint(&self.manifest.workspace.scan)?;
        let catalog = profile::measure("host.resolve", || {
            FactCatalog::resolve(
                files,
                self.policy.policy_fingerprint.clone(),
                selection,
                &self.ctx,
            )
        })
        .map_err(crate::graph_facts::map_error)?;
        self.ctx
            .check_output(
                catalog
                    .files
                    .len()
                    .saturating_add(catalog.resolutions.len()),
            )
            .map_err(crate::graph_facts::map_error)?;
        Ok(catalog)
    }
    fn restrict_domain(&self, bound: &mut BoundFileFacts) {
        if bound.facts.as_ref().is_some_and(|f| {
            if f.language == Language::Markdown {
                !self.policy.configuration.scope.documents()
            } else {
                !self.policy.configuration.scope.code()
            }
        }) {
            bound.facts = None;
            bound.node_entities.clear();
            bound.reference_entities.clear();
            bound.text_map.clear();
            bound.text_binding_digest = None;
        }
    }

    fn extract(
        &self,
        file: &FileRecord,
        inputs: &HashMap<PathBuf, Vec<u8>>,
        collector: &FactCapture,
        ids: &[String],
        ids_digest: &str,
    ) -> EngineResult<BoundFileFacts> {
        let path = file.relative_path.to_path_buf();
        let formats =
            FileFormat::from_path(&self.manifest.workspace.source_path(&file.relative_path))?;
        let enabled = (self.policy.configuration.scope.code()
            && formats
                .iter()
                .any(|f| matches!(f, FileFormat::Rust | FileFormat::Python)))
            || (self.policy.configuration.scope.documents()
                && formats.contains(&FileFormat::Markdown));
        if enabled {
            let bytes = inputs
                .get(&path)
                .ok_or_else(|| EngineError::resource_busy("graph source snapshot unavailable"))?;
            if Some(crate::utils::sha256_hex(bytes)) != file.snapshot.content_hash {
                return Err(EngineError::resource_busy("graph source revision changed"));
            }
            let decoded = crate::utils::decode_index_text(&formats, bytes)
                .ok_or_else(|| EngineError::invalid_argument("graph source decode failed"))?;
            let descriptor = self.manifest.workspace.index.descriptor().ok_or_else(|| {
                EngineError::storage_failure("graph build requires text descriptor")
            })?;
            let max_tokens = descriptor
                .model_for(crate::domain::ContentKind::Text)?
                .max_input_tokens;
            let chunks = super::input_budget::index_chunk_options(max_tokens, Some(&decoded));
            let source = TextSource {
                relative_path: file.relative_path.clone(),
                formats,
                text: decoded.into_owned(),
            };
            let (entities, facts) = collector.extract(
                &source,
                chunks,
                file.snapshot
                    .content_hash
                    .as_deref()
                    .ok_or_else(|| EngineError::storage_failure("missing source digest"))?,
            )?;
            let entities = extraction::bind_entities(file.id, entities)?;
            let mut expected = entities
                .iter()
                .map(|e| e.id.as_str().to_owned())
                .collect::<Vec<_>>();
            expected.sort();
            if expected != ids {
                return Err(EngineError::resource_busy(
                    "graph extraction differs from stored entity mapping",
                ));
            }
            if let Some(bound) = collector.bind(file, &entities, facts)? {
                return Ok(bound);
            }
        }
        BoundFileFacts::inventory_only(
            BoundRecord {
                path: path
                    .to_str()
                    .ok_or_else(|| EngineError::unsupported("non UTF-8 graph path"))?
                    .into(),
                file_id: file.id.get(),
                index_status: IndexStatus::Indexed,
                content_hash: file.snapshot.content_hash.clone(),
                size_bytes: file.snapshot.size_bytes,
                text_revision: self.state.text_revision,
                storage_generation: collector.generation.clone(),
                entity_count: ids.len() as u64,
                entity_ids_digest: ids_digest.into(),
            },
            &self.ctx,
        )
        .map_err(crate::graph_facts::map_error)
    }
    fn record(&self) -> EngineResult<BuildRecord> {
        Ok(BuildRecord {
            format_version: 1,
            graph_build_id: self.id.into(),
            operation_id: uuid::Uuid::new_v4().to_string(),
            attempt: 1,
            base_text_revision: self.state.text_revision,
            base_catalog_digest: self.state.catalog_digest.clone(),
            base_head: self.old_head.as_ref().map(|h| h.graph_build_id.clone()),
            scope: match self.policy.configuration.scope {
                GraphScope::Code => BuildScope::Code,
                GraphScope::Documents => BuildScope::Documents,
                GraphScope::Both => BuildScope::Both,
                GraphScope::Off => {
                    return Err(EngineError::invalid_argument("graph policy is off"));
                }
            },
            configuration: serde_json::to_value(&self.policy.configuration)
                .map_err(|_| EngineError::invalid_argument("invalid graph configuration"))?,
            policy_fingerprint: self.policy.policy_fingerprint.clone(),
            rules_fingerprint: rules_fingerprint()?,
            module_roots_fingerprint: fingerprint(&self.policy.configuration.module_roots)?,
            schema_fingerprint: self.schema.clone(),
            created_epoch_ms: graph_state::now(),
            import_budget: Some(ImportBudget::default()),
        })
    }
    fn commit(&self, record: &BuildRecord, catalog: &FactCatalog) -> EngineResult<()> {
        let path = self.home.join("graph/builds").join(self.id);
        write_record(&path.join("build.json"), record)?;
        attempt(
            &path,
            record,
            "registered",
            "not_applied",
            "not_selected",
            false,
            None,
        )?;
        let graph = self.import(record, catalog)?;
        self.resources.check(false)?;
        attempt(
            &path,
            record,
            "commit_pending",
            "unknown",
            "not_selected",
            self.ctx.cancellation.is_cancelled(),
            None,
        )?;
        if let Err(error) = graph.commit_build(&record.spec(), &self.ctx) {
            let known = match error.outcome {
                Some(GraphOutcome::Committed) => "committed",
                Some(GraphOutcome::Unknown | GraphOutcome::InTransaction) => "unknown",
                _ if error.kind == GraphErrorKind::RecoveryRequired => "unknown",
                _ => "not_applied",
            };
            let _ = attempt(
                &path,
                record,
                if known == "unknown" {
                    "unknown"
                } else {
                    "failed"
                },
                known,
                "not_selected",
                self.ctx.cancellation.is_cancelled(),
                Some(&format!("{:?}", error.kind)),
            );
            return Err(graph_error(&error));
        }
        attempt(
            &path,
            record,
            "checkpoint_pending",
            "committed",
            "not_selected",
            self.ctx.cancellation.is_cancelled(),
            None,
        )?;
        profile::measure("graph.checkpoint", || graph.checkpoint()).map_err(|e| graph_error(&e))?;
        attempt(
            &path,
            record,
            "confirmed",
            "committed",
            "not_selected",
            self.ctx.cancellation.is_cancelled(),
            None,
        )?;
        self.resources.check(false)?;
        Ok(())
    }
    fn import(&self, record: &BuildRecord, catalog: &FactCatalog) -> EngineResult<GraphDbStore> {
        let path = self.home.join("graph/builds").join(self.id);
        attempt(
            &path,
            record,
            "import_pending",
            "unknown",
            "not_selected",
            self.ctx.cancellation.is_cancelled(),
            None,
        )?;
        let failure = |error: zg_knowledge::graph::GraphError| {
            let unknown = error.kind == GraphErrorKind::RecoveryRequired
                || matches!(
                    error.outcome,
                    Some(GraphOutcome::Unknown | GraphOutcome::InTransaction)
                );
            let _ = attempt(
                &path,
                record,
                if unknown { "unknown" } else { "failed" },
                if unknown { "unknown" } else { "not_applied" },
                "not_selected",
                self.ctx.cancellation.is_cancelled(),
                Some(&format!("{:?}", error.kind)),
            );
            graph_error(&error)
        };
        let graph = profile::measure("graph.create", || {
            GraphDbStore::create_batched_build(path.join("db"), self.limits.clone())
        })
        .map_err(failure)?;
        let mut rejected = None;
        let budget = record
            .import_budget
            .as_ref()
            .ok_or_else(|| EngineError::storage_failure("missing import budget"))?;
        let imported = profile::measure("graph.import", || {
            graph.write_facts_batched_admitted(
                catalog,
                budget,
                &self.ctx,
                || {
                    self.resources.check(false).map_err(|error| {
                        rejected = Some(error);
                        zg_knowledge::graph::GraphError::from(
                            zg_knowledge::Error::ResourceExhausted,
                        )
                    })
                },
                |_| {},
            )
        });
        if let Err(error) = imported {
            let mapped = failure(error);
            return Err(rejected.unwrap_or(mapped));
        }
        if graph.read_facts(&self.ctx).map_err(failure)? != *catalog {
            return Err(EngineError::storage_failure(
                "candidate validation mismatch; recovery_required",
            ));
        }
        attempt(
            &path,
            record,
            "validated",
            "not_applied",
            "not_selected",
            self.ctx.cancellation.is_cancelled(),
            None,
        )?;
        Ok(graph)
    }
    /// Resume an interrupted pre-commit candidate in place. Reopens the existing
    /// database and continues the persisted batch prefix with the reconstructed
    /// fixed-input catalog. The immutable record and operation identity are
    /// supplied by the inspected caller rather than regenerated.
    fn resume(&self, record: &BuildRecord, catalog: &FactCatalog) -> EngineResult<Vec<String>> {
        let path = self.home.join("graph/builds").join(self.id);
        let graph = self.resume_import(&path, record, catalog)?;
        attempt(
            &path,
            record,
            "commit_pending",
            "unknown",
            "not_selected",
            self.ctx.cancellation.is_cancelled(),
            None,
        )?;
        if let Err(error) = graph.commit_build(&record.spec(), &self.ctx) {
            let known = match error.outcome {
                Some(GraphOutcome::Committed) => "committed",
                Some(GraphOutcome::Unknown | GraphOutcome::InTransaction) => "unknown",
                _ if error.kind == GraphErrorKind::RecoveryRequired => "unknown",
                _ => "not_applied",
            };
            let _ = attempt(
                &path,
                record,
                if known == "unknown" {
                    "unknown"
                } else {
                    "failed"
                },
                known,
                "not_selected",
                self.ctx.cancellation.is_cancelled(),
                Some(&format!("{:?}", error.kind)),
            );
            if known == "committed" {
                return Err(committed_recovery(
                    &path,
                    record,
                    self.ctx.cancellation.is_cancelled(),
                    &format!("{:?}", error.kind),
                ));
            }
            return Err(graph_error(&error));
        }
        #[cfg(test)]
        after_resume_commit(self.id, &self.operation);
        drop(graph);
        self.confirm_resume(&path, record)
    }

    fn resume_import(
        &self,
        path: &std::path::Path,
        record: &BuildRecord,
        catalog: &FactCatalog,
    ) -> EngineResult<GraphDbStore> {
        let budget = record
            .import_budget
            .as_ref()
            .ok_or_else(|| EngineError::storage_failure("missing import budget"))?;
        attempt(
            path,
            record,
            "import_pending",
            "unknown",
            "not_selected",
            self.ctx.cancellation.is_cancelled(),
            None,
        )?;
        let failure = |error: zg_knowledge::graph::GraphError| {
            let unknown = error.kind == GraphErrorKind::RecoveryRequired
                || matches!(
                    error.outcome,
                    Some(GraphOutcome::Unknown | GraphOutcome::InTransaction)
                );
            let _ = attempt(
                path,
                record,
                if unknown { "unknown" } else { "failed" },
                if unknown { "unknown" } else { "not_applied" },
                "not_selected",
                self.ctx.cancellation.is_cancelled(),
                Some(&format!("{:?}", error.kind)),
            );
            graph_error(&error)
        };
        let graph =
            GraphDbStore::open_import(path.join("db"), false, self.limits.clone(), &self.ctx)
                .map_err(failure)?;
        let mut rejected = None;
        let imported = graph.write_facts_batched_admitted(
            catalog,
            budget,
            &self.ctx,
            || {
                self.resources.check(false).map_err(|error| {
                    rejected = Some(error);
                    zg_knowledge::graph::GraphError::from(zg_knowledge::Error::ResourceExhausted)
                })
            },
            |_| {},
        );
        if let Err(error) = imported {
            let mapped = failure(error);
            return Err(rejected.unwrap_or(mapped));
        }
        if graph.read_facts(&self.ctx).map_err(failure)? != *catalog {
            return Err(EngineError::storage_failure(
                "candidate validation mismatch; recovery_required",
            ));
        }
        attempt(
            path,
            record,
            "validated",
            "not_applied",
            "not_selected",
            self.ctx.cancellation.is_cancelled(),
            None,
        )?;
        self.resources.check(false)?;
        Ok(graph)
    }

    fn confirm_resume(
        &self,
        path: &std::path::Path,
        record: &BuildRecord,
    ) -> EngineResult<Vec<String>> {
        let confirmed = (|| {
            let confirmation = self.operation.confirmation()?;
            let graph = GraphDbStore::open_import(
                path.join("db"),
                false,
                confirmation.limits(),
                confirmation.context(),
            )
            .map_err(|e| graph_error(&e))?;
            attempt(
                path,
                record,
                "checkpoint_pending",
                "committed",
                "not_selected",
                self.ctx.cancellation.is_cancelled(),
                None,
            )?;
            graph.checkpoint().map_err(|e| graph_error(&e))?;
            if graph
                .inspect_build(&record.spec(), confirmation.context())
                .map_err(|e| graph_error(&e))?
                .is_none()
            {
                return Err(EngineError::storage_failure(
                    "committed candidate confirmation recovery_required",
                ));
            }
            attempt(
                path,
                record,
                "confirmed",
                "committed",
                "not_selected",
                self.ctx.cancellation.is_cancelled(),
                None,
            )?;
            self.resources.check(false)?;
            Ok(confirmation_diagnostics(confirmation.original_stop()))
        })();
        confirmed.map_err(|error| {
            committed_recovery(
                path,
                record,
                self.ctx.cancellation.is_cancelled(),
                error.code(),
            )
        })
    }
    fn select(
        &self,
        record: &BuildRecord,
        catalog: &FactCatalog,
        work: GraphWorkMode,
    ) -> EngineResult<GraphIndexResult> {
        check_context(&self.ctx)?;
        self.resources.check(false)?;
        if GraphPolicy::read(&self.manifest.path)?
            .as_ref()
            .map(|p| &p.policy_fingerprint)
            != Some(&self.policy.policy_fingerprint)
            || graph_state::catalog_digest(&self.storage.list_files()?) != self.state.catalog_digest
            || !bound_matches(&self.storage, &self.files, catalog)?
        {
            return Err(EngineError::resource_busy("graph publication base changed"));
        }
        self.storage.close()?;
        let domains = self.domains(catalog)?;
        let head = GraphHead {
            format_version: 1,
            graph_build_id: self.id.into(),
            text_revision: self.state.text_revision,
            catalog_digest: self.state.catalog_digest.clone(),
            contract_version: "zg-relations-v1".into(),
            schema_fingerprint: self.schema.clone(),
            policy_fingerprint: self.policy.policy_fingerprint.clone(),
            domains: domains.clone(),
        };
        let path = self.home.join("graph/builds").join(self.id);
        check_context(&self.ctx)?;
        attempt(
            &path,
            record,
            "select_pending",
            "committed",
            "unknown",
            self.ctx.cancellation.is_cancelled(),
            None,
        )?;
        if let Err(error) = GraphHead::select(
            &self.home,
            &self.state,
            record.base_head.as_deref(),
            &self.policy.policy_fingerprint,
            &head,
        ) {
            if error.code() == EngineError::RESOURCE_BUSY {
                // A failed compare-and-swap made no pointer change. Preserve
                // that known outcome rather than declaring selection unknown.
                attempt(
                    &path,
                    record,
                    "confirmed",
                    "committed",
                    "not_selected",
                    self.ctx.cancellation.is_cancelled(),
                    Some(error.code()),
                )?;
            }
            return Err(error);
        }
        let late = self.ctx.cancellation.is_cancelled();
        let _ = attempt(
            &path,
            record,
            if work == GraphWorkMode::TextRebuild {
                "confirmed"
            } else {
                "selected"
            },
            "committed",
            if work == GraphWorkMode::TextRebuild {
                "not_selected"
            } else {
                "selected"
            },
            late,
            None,
        );
        let mut diagnostics = Vec::new();
        if late && work != GraphWorkMode::TextRebuild {
            diagnostics.push("cancellation_arrived_after_selection".into());
        }
        if catalog.files.len() != self.files.len() {
            diagnostics.push("unsupported_graph_path".into());
        }
        Ok(GraphIndexResult {
            text_published: work.text_published(),
            graph_candidate_committed: true,
            graph_selected: work != GraphWorkMode::TextRebuild,
            graph_build_id: Some(self.id.into()),
            mode: work.label().into(),
            domains: work.result_domains(domains),
            diagnostics,
        })
    }
    fn domains(&self, catalog: &FactCatalog) -> EngineResult<BTreeMap<String, GraphDomainResult>> {
        coverage::summarize(self.manifest, self.policy, &self.files, catalog, &self.ctx)
    }
}

/// Close/reopen verification before the manifest makes a candidate generation
/// externally selected. This helper never chooses or repairs anything.
#[cfg(test)]
pub(super) fn verify_prepared_graph(manifest: &WorkspaceManifest) -> EngineResult<bool> {
    verify_prepared_graph_with(manifest, &context(None), GraphLimits::default())
}
fn verify_prepared_graph_with(
    manifest: &WorkspaceManifest,
    context: &ProcessingContext,
    limits: GraphLimits,
) -> EngineResult<bool> {
    let _profile = profile::BuildProfile::start("host.manifest_verify");
    let _stage = profile::stage("manifest.verify");
    let home = manifest.storage_home();
    let Some(head) = GraphHead::read(&home)? else {
        return Ok(false);
    };
    let path = home.join("graph/builds").join(&head.graph_build_id);
    let record: BuildRecord = read_record(&path.join("build.json"))?
        .ok_or_else(|| EngineError::storage_failure("missing graph build input"))?;
    let graph = GraphDbStore::open(path.join("db"), true, limits).map_err(|e| graph_error(&e))?;
    Ok(graph
        .inspect_build(&record.spec(), context)
        .map_err(|e| graph_error(&e))?
        .is_some())
}
pub(super) async fn begin_manifest_selection(
    manifest: &WorkspaceManifest,
    result: Option<&GraphIndexResult>,
    cancellation: Option<CancellationToken>,
    admission: Arc<crate::workspace::lock::FileLock>,
) -> EngineResult<bool> {
    let manifest = manifest.clone();
    let result = result.cloned();
    control::run(admission, context(cancellation), move |operation| {
        begin_manifest_selection_blocking(&manifest, result.as_ref(), &operation)
    })
    .await
}
fn begin_manifest_selection_blocking(
    manifest: &WorkspaceManifest,
    result: Option<&GraphIndexResult>,
    operation: &control::Operation,
) -> EngineResult<bool> {
    let check = || check_context(&operation.context);
    check()?;
    let Some(result) = result.filter(|r| r.graph_candidate_committed) else {
        return Ok(false);
    };
    let home = manifest.storage_home();
    let Some(head) = GraphHead::read(&home)? else {
        return Ok(false);
    };
    if result.graph_build_id.as_deref() != Some(head.graph_build_id.as_str()) {
        return Err(EngineError::resource_busy("candidate graph HEAD changed"));
    }
    resources::CandidateGuard::new(manifest, &head.graph_build_id).check(false)?;
    if !verify_prepared_graph_with(manifest, &operation.context, operation.limits())? {
        return Err(EngineError::storage_failure(
            "candidate graph has no commit proof",
        ));
    }
    check()?;
    let path = home.join("graph/builds").join(head.graph_build_id);
    let record: BuildRecord = read_record(&path.join("build.json"))?
        .ok_or_else(|| EngineError::storage_failure("missing graph build input"))?;
    attempt(
        &path,
        &record,
        "select_pending",
        "committed",
        "unknown",
        false,
        None,
    )?;
    Ok(true)
}
pub(super) fn confirm_manifest_selection(
    manifest: &WorkspaceManifest,
    result: Option<&mut GraphIndexResult>,
    cancelled: bool,
) -> EngineResult<()> {
    let Some(result) = result.filter(|r| r.graph_candidate_committed) else {
        return Ok(());
    };
    let home = manifest.storage_home();
    let Some(head) = GraphHead::read(&home)? else {
        return Ok(());
    };
    let active = crate::workspace::manifest::read_workspace_manifest(&manifest.path)?;
    if active.as_ref().and_then(|m| m.storage_generation.as_ref())
        != manifest.storage_generation.as_ref()
        || result.graph_build_id.as_deref() != Some(head.graph_build_id.as_str())
    {
        return Err(EngineError::storage_failure(
            "manifest selection recovery_required",
        ));
    }
    let path = home.join("graph/builds").join(&head.graph_build_id);
    let record: BuildRecord = read_record(&path.join("build.json"))?
        .ok_or_else(|| EngineError::storage_failure("graph selection recovery_required"))?;
    result.graph_selected = true;
    result.domains = head.domains;
    if cancelled {
        result
            .diagnostics
            .push("cancellation_arrived_after_selection".into());
    }
    let _ = attempt(
        &path,
        &record,
        "selected",
        "committed",
        "selected",
        cancelled,
        None,
    );
    Ok(())
}

#[cfg(test)]
#[path = "graph/retention_tests.rs"]
mod retention_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace::lock::{LockMode, acquire_read_write_lock};

    #[tokio::test]
    async fn graph_worker_retains_real_admission_after_async_caller_is_aborted() {
        let dir = tempfile::tempdir().expect("workspace");
        let path = dir.path().join("admission.lock");
        let admission = Arc::new(
            acquire_read_write_lock(&path, LockMode::Write, "original").expect("write admission"),
        );
        let (entered, started) = tokio::sync::oneshot::channel();
        let (release, wait) = std::sync::mpsc::channel();
        let parent = tokio::spawn(in_worker(admission, move || {
            let _ = entered.send(());
            let _ = wait.recv();
            Ok(())
        }));
        tokio::time::timeout(Duration::from_secs(5), started)
            .await
            .expect("worker starts")
            .expect("entry signal");
        parent.abort();
        assert!(parent.await.expect_err("caller cancelled").is_cancelled());
        let blocked = acquire_read_write_lock(&path, LockMode::Write, "competitor");
        // Release even if an assertion fails, so runtime shutdown cannot hang.
        release.send(()).expect("worker still runs");
        assert_eq!(
            blocked
                .expect_err("native work still owns admission")
                .code(),
            EngineError::RESOURCE_BUSY
        );
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if let Ok(lock) = acquire_read_write_lock(&path, LockMode::Write, "after worker") {
                    drop(lock);
                    break;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("admission is released after native work returns");
    }
}
