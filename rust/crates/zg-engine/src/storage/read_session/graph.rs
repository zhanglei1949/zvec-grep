//! Immutable graph owners follow the existing text read lease. This checks the
//! published index view, not current filesystem contents: callers must verify
//! source evidence before reporting query results.
use crate::{
    EngineError, EngineResult,
    storage::{
        IndexStore,
        graph_state::{self, BuildRecord, GraphHead, TextPhase, TextState},
    },
    workspace::{graph_policy::GraphPolicy, manifest::WorkspaceManifest},
};
use serde::Serialize;
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Arc, Mutex, OnceLock, Weak},
};
use zg_knowledge::{
    graph::{GraphDbStore, GraphLimits},
    processing::ProcessingContext,
    relations::bound::FactCatalog,
};

#[derive(Serialize)]
struct Selection {
    head: GraphHead,
    state: TextState,
    scan: String,
    build: String,
}
fn unavailable() -> EngineError {
    EngineError::resource_busy("selected graph is missing, stale or incompatible")
}
fn failure(error: &zg_knowledge::graph::GraphError) -> EngineError {
    use zg_knowledge::graph::{GraphErrorKind, GraphOutcome};
    if error.kind == GraphErrorKind::RecoveryRequired
        || matches!(
            error.outcome,
            Some(GraphOutcome::Unknown | GraphOutcome::InTransaction)
        )
    {
        return EngineError::storage_failure("graph read recovery_required");
    }
    match error.kind {
        GraphErrorKind::Cancelled => EngineError::cancelled("graph read cancelled"),
        GraphErrorKind::Deadline => EngineError::deadline_exceeded("graph read deadline exceeded"),
        GraphErrorKind::Conflict => EngineError::resource_busy("graph read conflict"),
        GraphErrorKind::ResourceExhausted => {
            EngineError::resource_busy("graph read budget exhausted")
        }
        GraphErrorKind::InvalidInput => {
            EngineError::invalid_argument("graph read input or budget rejected")
        }
        _ => {
            EngineError::storage_failure(format!("graph read {:?} at {}", error.kind, error.stage))
        }
    }
}
impl Selection {
    fn read(manifest: &WorkspaceManifest) -> EngineResult<Self> {
        let active = crate::workspace::manifest::read_workspace_manifest(&manifest.path)?
            .ok_or_else(unavailable)?;
        if active.storage_generation != manifest.storage_generation
            || active.workspace.scan != manifest.workspace.scan
            || active.workspace.root != manifest.workspace.root
        {
            return Err(unavailable());
        }
        if !manifest
            .storage_generation
            .as_deref()
            .is_some_and(graph_state::valid_uuid)
        {
            return Err(unavailable());
        }
        let home = manifest.storage_home();
        let head = GraphHead::read(&home)?.ok_or_else(unavailable)?;
        let state = TextState::read(&home)?.ok_or_else(unavailable)?;
        let policy = GraphPolicy::read(&manifest.path)?.ok_or_else(unavailable)?;
        if !policy.configuration.scope.enabled()
            || head.policy_fingerprint != policy.policy_fingerprint
            || state.state != TextPhase::Stable
            || state.text_revision != head.text_revision
            || state.catalog_digest != head.catalog_digest
            || head.schema_fingerprint
                != zg_knowledge::graph::schema::build_fingerprint()
                    .map_err(crate::graph_facts::map_error)?
        {
            return Err(unavailable());
        }
        let build = home.join("graph/builds").join(&head.graph_build_id);
        let record: BuildRecord =
            graph_state::read_record(&build.join("build.json"))?.ok_or_else(unavailable)?;
        let attempt: serde_json::Value =
            graph_state::read_record(&build.join("attempt.json"))?.ok_or_else(unavailable)?;
        if attempt["operationId"] != record.operation_id
            || attempt["attempt"] != record.attempt
            || attempt["stage"] != "selected"
            || attempt["commitEffect"] != "committed"
            || attempt["selectionEffect"] != "selected"
        {
            return Err(EngineError::resource_busy(
                "graph selection requires explicit reconciliation",
            ));
        }
        Ok(Self {
            head,
            state,
            scan: graph_state::fingerprint(&manifest.workspace.scan)?,
            build: graph_state::fingerprint(&record)?,
        })
    }
    fn key(&self) -> EngineResult<String> {
        graph_state::fingerprint(self)
    }
}

pub(super) struct GraphReadSession {
    // Only Database is shared; each read uses an exclusive connection and a
    // fresh cancellation handle. Transactions never cross an await.
    owner: Arc<GraphDbStore>,
    key: String,
    // Only deduplicates native directory owners across engine caches. All
    // lifetime/retirement and residency still belongs to ReadSessionCache.
    _slot: Arc<Mutex<Weak<GraphDbStore>>>,
}
type OwnerSlot = Arc<Mutex<Weak<GraphDbStore>>>;
static OWNERS: OnceLock<Mutex<HashMap<PathBuf, OwnerSlot>>> = OnceLock::new();

fn owner(path: &std::path::Path) -> EngineResult<(Arc<GraphDbStore>, OwnerSlot)> {
    let slot = {
        let mut owners = super::lock(OWNERS.get_or_init(Mutex::default));
        owners.retain(|_, slot| Arc::strong_count(slot) > 1);
        Arc::clone(owners.entry(path.to_path_buf()).or_default())
    };
    let instance = {
        // No native open or close under the global owner-map lock.
        let mut weak = super::lock(&slot);
        if let Some(owner) = weak.upgrade() {
            owner
        } else {
            let owner = Arc::new(
                GraphDbStore::open(path, true, GraphLimits::default()).map_err(|e| failure(&e))?,
            );
            *weak = Arc::downgrade(&owner);
            owner
        }
    };
    Ok((instance, slot))
}
impl GraphReadSession {
    pub(super) fn key(manifest: &WorkspaceManifest) -> EngineResult<String> {
        Selection::read(manifest)?.key()
    }
    pub(super) fn open(
        manifest: &WorkspaceManifest,
        expected_key: &str,
        _storage: &IndexStore,
    ) -> EngineResult<Self> {
        let selected = Selection::read(manifest)?;
        if selected.key()? != expected_key {
            return Err(unavailable());
        }
        let mut path = manifest.path.clone();
        for part in [
            "generations",
            manifest
                .storage_generation
                .as_deref()
                .ok_or_else(unavailable)?,
            "graph",
            "builds",
            &selected.head.graph_build_id,
            "db",
        ] {
            path.push(part);
            let meta = std::fs::symlink_metadata(&path).map_err(|_| unavailable())?;
            if !meta.is_dir() || meta.file_type().is_symlink() {
                return Err(unavailable());
            }
        }
        let record: BuildRecord =
            graph_state::read_record(&path.parent().ok_or_else(unavailable)?.join("build.json"))?
                .ok_or_else(unavailable)?;
        if record.graph_build_id != selected.head.graph_build_id
            || record.base_text_revision != selected.state.text_revision
            || record.base_catalog_digest != selected.state.catalog_digest
            || record.policy_fingerprint != selected.head.policy_fingerprint
            || record.schema_fingerprint != selected.head.schema_fingerprint
            || record.rules_fingerprint != graph_state::rules_fingerprint()?
        {
            return Err(unavailable());
        }
        let (owner, slot) = owner(&path)?;
        let session = Self {
            owner,
            key: expected_key.into(),
            _slot: slot,
        };
        Ok(session)
    }
    pub(super) fn query_owner(&self, limits: GraphLimits) -> EngineResult<GraphDbStore> {
        self.owner.read_session(limits).map_err(|e| failure(&e))
    }
    pub(super) fn query_stamp(
        &self,
        manifest: &WorkspaceManifest,
    ) -> EngineResult<zg_knowledge::query::ViewStamp> {
        let selected = Selection::read(manifest)?;
        if selected.key()? != self.key {
            return Err(unavailable());
        }
        let record: BuildRecord = graph_state::read_record(
            &manifest
                .storage_home()
                .join("graph/builds")
                .join(&selected.head.graph_build_id)
                .join("build.json"),
        )?
        .ok_or_else(unavailable)?;
        Ok(zg_knowledge::query::ViewStamp {
            workspace_id: graph_state::fingerprint(&manifest.path)?,
            generation: manifest
                .storage_generation
                .clone()
                .ok_or_else(unavailable)?,
            text_revision: selected.state.text_revision,
            graph: Some(zg_knowledge::query::GraphStamp {
                build_id: selected.head.graph_build_id,
                schema: selected.head.schema_fingerprint,
                rules: record.rules_fingerprint,
                configuration: selected.head.policy_fingerprint,
                catalog_revision: selected.head.catalog_digest,
            }),
        })
    }
    pub(super) fn validate_query(
        &self,
        manifest: &WorkspaceManifest,
        storage: &IndexStore,
        limits: GraphLimits,
        context: &ProcessingContext,
        code: bool,
    ) -> EngineResult<()> {
        let _qualification = zg_knowledge::graph::profile::stage("qualification.validate");
        let selected = Selection::read(manifest)?;
        if selected.key()? != self.key {
            return Err(unavailable());
        }
        if code
            && !selected
                .head
                .domains
                .get("code")
                .is_some_and(|d| d.ready == crate::api::index::graph::GraphReady::Ready)
        {
            return Err(EngineError::resource_busy(
                "code graph domain is not_ready; inspect source coverage",
            ));
        }
        let record: BuildRecord = graph_state::read_record(
            &manifest
                .storage_home()
                .join("graph/builds")
                .join(&selected.head.graph_build_id)
                .join("build.json"),
        )?
        .ok_or_else(unavailable)?;
        let graph = self.query_owner(limits)?;
        let proof_stage = zg_knowledge::graph::profile::stage("qualification.graph_proof");
        if graph
            .inspect_published(&record.spec(), context)
            .map_err(|e| failure(&e))?
            .is_none()
        {
            return Err(unavailable());
        }
        drop(proof_stage);
        let _text = zg_knowledge::graph::profile::stage("qualification.text_binding");
        let files = storage.list_files()?;
        if graph_state::catalog_digest(&files) != selected.state.catalog_digest {
            return Err(unavailable());
        }
        let files_by_id = files
            .iter()
            .map(|file| (file.id.get(), file))
            .collect::<HashMap<_, _>>();
        if files_by_id.len() != files.len() {
            return Err(unavailable());
        }
        let mut after = String::new();
        let mut count = 0usize;
        loop {
            context.check().map_err(crate::graph_facts::map_error)?;
            let page = graph
                .source_records(&after, context)
                .map_err(|e| failure(&e))?;
            if page.is_empty() {
                break;
            }
            for source in &page {
                zg_knowledge::graph::profile::count("qualification.sources", 1);
                count += 1;
                context
                    .check_output(count)
                    .map_err(crate::graph_facts::map_error)?;
                verify_source(manifest, storage, source, &files_by_id, context, code)?;
            }
            after.clone_from(&page.last().ok_or_else(unavailable)?.record.path);
        }
        let expected = files
            .iter()
            .filter(|f| {
                f.relative_path.to_path_buf().to_str().is_some_and(|p| {
                    !p.contains(['\\', ':', '\0'])
                        && p.split('/').all(|c| !matches!(c, "" | "." | ".."))
                })
            })
            .count();
        if count != expected || Selection::read(manifest)?.key()? != self.key {
            return Err(unavailable());
        }
        Ok(())
    }
    pub(super) fn read(
        &self,
        manifest: &WorkspaceManifest,
        storage: &IndexStore,
        limits: GraphLimits,
        context: &ProcessingContext,
    ) -> EngineResult<FactCatalog> {
        if Self::key(manifest)? != self.key {
            return Err(unavailable());
        }
        let catalog = self
            .owner
            .read_session(limits)
            .map_err(|e| failure(&e))?
            .read_facts(context)
            .map_err(|e| failure(&e))?;
        let files = storage.list_files()?;
        let selection = Selection::read(manifest)?;
        if selection.key()? != self.key
            || graph_state::catalog_digest(&files) != selection.state.catalog_digest
            || catalog.selection_fingerprint != selection.scan
            || catalog.config_fingerprint != selection.head.policy_fingerprint
            || catalog.files.iter().any(|file| {
                manifest.storage_generation.as_deref()
                    != Some(file.record.storage_generation.as_str())
            })
            || !graph_state::bound_matches(storage, &files, &catalog)?
        {
            return Err(unavailable());
        }
        Ok(catalog)
    }
}

fn verify_source(
    manifest: &WorkspaceManifest,
    storage: &IndexStore,
    source: &zg_knowledge::query::SourceRecord,
    files: &HashMap<u32, &crate::domain::FileRecord>,
    context: &ProcessingContext,
    code: bool,
) -> EngineResult<()> {
    use zg_knowledge::relations::Language;
    let r = &source.record;
    let file = files
        .get(&r.file_id)
        .copied()
        .filter(|f| f.relative_path.as_ref() == std::path::Path::new(&r.path))
        .ok_or_else(unavailable)?;
    let identities =
        usize::try_from(file.index_status.entity_count()).map_err(|_| unavailable())?;
    context
        .check_output(identities)
        .map_err(crate::graph_facts::map_error)?;
    context
        .check_input(identities.saturating_mul(32))
        .map_err(crate::graph_facts::map_error)?;
    zg_knowledge::graph::profile::count("qualification.identity_queries", 1);
    let ids = zg_knowledge::graph::profile::measure("qualification.entity_ids", || {
        storage.file_entity_ids(file)
    })?;
    if !file.index_status.is_indexed()
        || file.snapshot.content_hash != r.content_hash
        || file.snapshot.size_bytes != r.size_bytes
        || r.storage_generation != manifest.storage_generation.as_deref().unwrap_or_default()
        || zg_knowledge::relations::bound::entity_ids_digest(&ids)
            .map_err(crate::graph_facts::map_error)?
            != r.entity_ids_digest
    {
        return Err(unavailable());
    }
    if code {
        let formats = crate::domain::FileFormat::from_path(
            &manifest.workspace.source_path(&file.relative_path),
        )?;
        let expected = if formats.contains(&crate::domain::FileFormat::Rust) {
            Some(Language::Rust)
        } else if formats.contains(&crate::domain::FileFormat::Python) {
            Some(Language::Python)
        } else {
            None
        };
        if expected.is_some() && (source.language != expected || !source.complete) {
            return Err(unavailable());
        }
    }
    if source.language.is_some()
        && !crate::graph_facts::text_binding::verify(
            storage,
            &ids,
            &r.path,
            &source.text_map,
            source.text_binding_digest.as_deref(),
        )?
    {
        return Err(unavailable());
    }
    Ok(())
}
