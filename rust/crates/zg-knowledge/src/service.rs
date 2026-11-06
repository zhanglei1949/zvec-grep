//! Application-facing graph operations. Publication stays with the application.
use crate::{
    graph::{
        GraphDbStore, GraphLimits, GraphResult, ImportBudget,
        build::{BuildSpec, ConfirmedBuild},
    },
    processing::ProcessingContext,
    query::{
        AugmentResult, ByteSpan, Direction, EntityRecord, EvidenceProvider, ExploreRequest,
        ExploreResult, GraphQueryMode, GraphReader, KgReadView, Locator, NeighborRead, QueryResult,
        RankedHits, ReadViewProvider, ReadViewRequest, ReferencePartition, ReferenceRecord,
        RelationKind, RelationRecord, RequestContext, ScopeResolution, ScopeSpec,
        ScopeTextProvider, SemanticPartition, SemanticQuery, SourceRecord,
    },
    relations::bound::FactCatalog,
};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

/// Opaque read owner. It exposes KG records, not a database or connection.
#[derive(Clone)]
pub struct GraphHandle(Arc<GraphDbStore>);
impl std::fmt::Debug for GraphHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("GraphHandle")
    }
}
impl GraphHandle {
    pub fn open(path: impl AsRef<Path>, limits: GraphLimits) -> GraphResult<Self> {
        Ok(Self(Arc::new(GraphDbStore::open(path, true, limits)?)))
    }
    pub fn read_session(&self, limits: GraphLimits) -> GraphResult<Self> {
        Ok(Self(Arc::new(self.0.read_session(limits)?)))
    }
    pub fn inspect_published(
        &self,
        spec: &BuildSpec,
        context: &ProcessingContext,
    ) -> GraphResult<Option<ConfirmedBuild>> {
        self.0.inspect_published(spec, context)
    }
    pub fn source_records(
        &self,
        after: &str,
        context: &ProcessingContext,
    ) -> GraphResult<Vec<SourceRecord>> {
        self.0.source_records(after, context)
    }
    pub fn read_facts(&self, context: &ProcessingContext) -> GraphResult<FactCatalog> {
        self.0.read_facts(context)
    }
    pub fn read_build_facts(
        &self,
        spec: &BuildSpec,
        context: &ProcessingContext,
    ) -> GraphResult<Option<FactCatalog>> {
        self.0.read_build_facts(spec, context)
    }
    pub fn find_source(
        &self,
        id: &str,
        context: &RequestContext,
    ) -> QueryResult<Option<SourceRecord>> {
        self.0.find_source(id, context)
    }
}
impl GraphReader for GraphHandle {
    fn neighbors_after(
        &self,
        read: &NeighborRead<'_>,
        ctx: &RequestContext,
    ) -> QueryResult<Vec<RelationRecord>> {
        self.0.neighbors_after(read, ctx)
    }
    fn semantic_selection(
        &self,
        q: &SemanticQuery,
        c: &RequestContext,
    ) -> QueryResult<Option<crate::semantic::SemanticCatalog>> {
        self.0.semantic_selection(q, c)
    }
    fn semantic_object(
        &self,
        query: &SemanticQuery,
        context: &RequestContext,
    ) -> QueryResult<Option<crate::semantic::SemanticCatalog>> {
        self.0.semantic_object(query, context)
    }
    fn source_nodes(&self, ids: &[String], ctx: &RequestContext) -> QueryResult<Vec<EntityRecord>> {
        self.0.source_nodes(ids, ctx)
    }
    fn semantic_page(
        &self,
        p: SemanticPartition,
        s: &str,
        o: u64,
        c: &RequestContext,
    ) -> QueryResult<crate::semantic::SemanticCatalog> {
        self.0.semantic_page(p, s, o, c)
    }
    fn span_nodes(
        &self,
        s: &str,
        p: ByteSpan,
        o: u64,
        c: &RequestContext,
    ) -> QueryResult<Vec<EntityRecord>> {
        self.0.span_nodes(s, p, o, c)
    }
    fn locate(&self, l: &Locator, o: u64, c: &RequestContext) -> QueryResult<Vec<EntityRecord>> {
        self.0.locate(l, o, c)
    }
    fn get_nodes(&self, ids: &[String], c: &RequestContext) -> QueryResult<Vec<EntityRecord>> {
        self.0.get_nodes(ids, c)
    }
    fn neighbors(
        &self,
        ids: &[String],
        k: RelationKind,
        d: Direction,
        o: u64,
        c: &RequestContext,
    ) -> QueryResult<Vec<RelationRecord>> {
        self.0.neighbors(ids, k, d, o, c)
    }
    fn references(
        &self,
        partition: ReferencePartition,
        owner: Option<&str>,
        source: Option<&str>,
        id: Option<&str>,
        offset: u64,
        context: &RequestContext,
    ) -> QueryResult<Vec<ReferenceRecord>> {
        self.0
            .references(partition, owner, source, id, offset, context)
    }
    fn source(&self, s: &str, c: &RequestContext) -> QueryResult<SourceRecord> {
        self.0.source(s, c)
    }
}

#[derive(Clone, Copy, Debug)]
pub enum BuildStage {
    Creating,
    Importing,
    Validated,
    CommitPending,
    Committed,
    CheckpointPending,
    Confirmed,
}

pub struct BuildRequest<'a> {
    pub path: &'a Path,
    pub spec: BuildSpec,
    pub catalog: &'a FactCatalog,
    pub budget: &'a ImportBudget,
    pub limits: GraphLimits,
    pub resume: bool,
}
pub struct BuildResult {
    pub receipt: ConfirmedBuild,
    pub logical_sources: usize,
    pub reused_sources: usize,
    pub rewritten_rows: u64,
    pub physical_full_rebuild: bool,
    pub elapsed_ms: u64,
    pub peak_native_segment_bytes: u64,
}
pub struct CandidateRequest<'a> {
    pub path: &'a Path,
    pub spec: &'a BuildSpec,
    pub limits: GraphLimits,
}
pub struct BuildInspection {
    pub receipt: Option<ConfirmedBuild>,
    pub selected: bool,
}
pub struct DiscardRequest {
    pub path: PathBuf,
    pub spec: BuildSpec,
    pub selected_build_id: Option<String>,
    /// The integrating application holds exclusive admission and has drained all leases.
    pub exclusive_cleanup: bool,
    pub limits: GraphLimits,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct KgService;
impl KgService {
    pub fn open_view<P: ReadViewProvider>(
        &self,
        provider: &P,
        req: &ReadViewRequest,
        ctx: &RequestContext,
    ) -> QueryResult<KgReadView<P::Lease>> {
        provider.open_view(req, ctx)
    }
    pub fn explore<L, E: EvidenceProvider<L>>(
        &self,
        view: &KgReadView<L>,
        evidence: &E,
        req: &ExploreRequest,
        ctx: &RequestContext,
    ) -> QueryResult<ExploreResult> {
        crate::query::explore(view.graph_handle()?, evidence, view, req, ctx)
    }
    pub fn augment<L, E: EvidenceProvider<L>>(
        &self,
        view: &KgReadView<L>,
        evidence: &E,
        hits: &RankedHits,
        mode: &GraphQueryMode,
        ctx: &RequestContext,
    ) -> QueryResult<AugmentResult> {
        crate::query::augment(view.graph_handle()?, evidence, view, hits, mode, ctx)
    }
    pub fn resolve_scope<L, T: ScopeTextProvider>(
        &self,
        view: &KgReadView<L>,
        text: &T,
        req: &ScopeSpec,
        ctx: &RequestContext,
    ) -> QueryResult<ScopeResolution> {
        crate::query::resolve_scope(view.graph_handle()?, text, view, req, ctx)
    }

    pub fn build_candidate(
        &self,
        req: BuildRequest<'_>,
        ctx: &ProcessingContext,
        mut admit: impl FnMut() -> GraphResult<()>,
        mut observe: impl FnMut(BuildStage) -> GraphResult<()>,
    ) -> GraphResult<BuildResult> {
        let start = std::time::Instant::now();
        ctx.check()?;
        admit()?;
        observe(BuildStage::Creating)?;
        let store = if req.resume {
            GraphDbStore::open_import(req.path, false, req.limits, ctx)?
        } else {
            GraphDbStore::create_batched_build(req.path, req.limits)?
        };
        observe(BuildStage::Importing)?;
        let progress =
            store.write_facts_batched_admitted(req.catalog, req.budget, ctx, &mut admit, |_| {})?;
        ctx.check()?;
        observe(BuildStage::Validated)?;
        admit()?;
        observe(BuildStage::CommitPending)?;
        let receipt = store.commit_build(&req.spec, ctx)?;
        let confirmed = (|| {
            observe(BuildStage::Committed)?;
            admit()?;
            observe(BuildStage::CheckpointPending)?;
            store.checkpoint()?;
            observe(BuildStage::Confirmed)?;
            Ok(BuildResult {
                receipt,
                logical_sources: req.catalog.files.len(),
                reused_sources: 0,
                rewritten_rows: progress.total_rows,
                physical_full_rebuild: true,
                elapsed_ms: u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX),
                peak_native_segment_bytes: store.peak_native_segment_bytes()?,
            })
        })();
        confirmed.map_err(|mut e: crate::graph::GraphError| {
            e.outcome = Some(crate::graph::GraphOutcome::Committed);
            e
        })
    }
    pub fn validate_candidate(
        &self,
        req: CandidateRequest<'_>,
        ctx: &ProcessingContext,
    ) -> GraphResult<Option<ConfirmedBuild>> {
        GraphHandle::open(req.path, req.limits)?.inspect_published(req.spec, ctx)
    }
    pub fn inspect_build(
        &self,
        req: CandidateRequest<'_>,
        selected_build_id: Option<&str>,
        ctx: &ProcessingContext,
    ) -> GraphResult<BuildInspection> {
        let store = GraphDbStore::open_import(req.path, true, req.limits, ctx)?;
        let receipt = store.inspect_build(req.spec, ctx)?;
        Ok(BuildInspection {
            selected: receipt.is_some()
                && selected_build_id == Some(req.spec.graph_build_id.as_str()),
            receipt,
        })
    }
    pub fn discard_candidate(
        &self,
        req: DiscardRequest,
        ctx: &ProcessingContext,
    ) -> GraphResult<()> {
        ctx.check()?;
        if !req.exclusive_cleanup
            || req.selected_build_id.as_deref() == Some(req.spec.graph_build_id.as_str())
        {
            return Err(crate::graph::GraphError::local(
                crate::graph::GraphErrorKind::Conflict,
            ));
        }
        // An exclusive native open prevents deletion while any native owner survives.
        let store = GraphDbStore::open_import(&req.path, false, req.limits, ctx)?;
        let _ = store.inspect_build(&req.spec, ctx)?;
        drop(store);
        std::fs::remove_dir_all(req.path)?;
        Ok(())
    }
}
