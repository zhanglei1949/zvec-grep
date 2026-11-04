//! Required graph scope is resolved before models, authorization or recall.
mod storage;
use super::{
    FileLock, QueryControl, QueryStopReason, ReadSessionLease, WorkspaceManifest,
    augmentation::{GraphExecution, text_proof},
    find_nearest_workspace, kg, qualify, query_error, read_workspace_manifest, try_home_read,
};
use crate::{
    EngineError, EngineResult,
    api::context::{
        ContextOptions, ContextResult,
        graph::GraphQueryMode,
        graph_result::{GraphScope, GraphStatus, GraphSupplement},
    },
    domain::{Content, Entity, EntityId, EntityMetadata, Range},
    pipelines::indexed_search::pipeline::{SearchPlan, resolve_filtered_file_ids},
};
use std::{
    collections::BTreeMap,
    sync::{Arc, mpsc},
    time::{Duration, Instant},
};
pub(in crate::pipelines) use storage::ScopedStorage;

pub(in crate::pipelines) struct PreparedScope {
    pub resolution: kg::ScopeResolution,
    pub units: Vec<kg::ScopeUnit>,
    pub entities: BTreeMap<String, Entity>,
    evidence: Vec<crate::api::context::graph_result::GraphEvidence>,
}
const MAX_SCOPE_BODY_BYTES: u64 = 256 << 20;
struct Text<'a> {
    store: &'a crate::storage::IndexStore,
    bytes: std::cell::Cell<u64>,
    entities: std::cell::RefCell<BTreeMap<String, Entity>>,
}
impl kg::ScopeTextProvider for Text<'_> {
    fn text_entities(
        &self,
        ids: &[String],
        ctx: &kg::RequestContext,
    ) -> kg::QueryResult<Vec<kg::ScopeTextEntity>> {
        ctx.check()?;
        self.store
            .graph_entities(ids)
            .map_err(|_| {
                kg::QueryError::new(
                    kg::QueryErrorKind::ViewMismatch,
                    "scope",
                    "text entity unavailable",
                )
            })?
            .into_iter()
            .map(|entity| {
                let metadata = text_entity(&entity, ctx)?;
                let used = self
                    .bytes
                    .get()
                    .checked_add(metadata.content_bytes)
                    .filter(|n| *n <= MAX_SCOPE_BODY_BYTES)
                    .ok_or_else(|| {
                        kg::QueryError::new(
                            kg::QueryErrorKind::ResourceExhausted,
                            "scope",
                            "scope body budget exhausted",
                        )
                    })?;
                self.bytes.set(used);
                self.entities
                    .borrow_mut()
                    .insert(entity.id.as_str().to_owned(), entity);
                Ok(metadata)
            })
            .collect()
    }
}
fn text_entity(entity: &Entity, ctx: &kg::RequestContext) -> kg::QueryResult<kg::ScopeTextEntity> {
    if EntityId::new(entity.file_id, &entity.content, entity.source_range)
        .ok()
        .as_ref()
        != Some(&entity.id)
    {
        return Err(kg::QueryError::new(
            kg::QueryErrorKind::ViewMismatch,
            "scope",
            "text identity changed",
        ));
    }
    let Content::Text(body) = &entity.content else {
        return Err(kg::QueryError::new(
            kg::QueryErrorKind::Unsupported,
            "scope",
            "scope_non_text_body",
        ));
    };
    let mut fragments = Vec::new();
    for fragment in &entity.fragments {
        ctx.check()?;
        let span = match fragment.range {
            Range::Full => kg::ByteSpan {
                start: 0,
                end: body.len() as u64,
            },
            Range::Byte(r) => kg::ByteSpan {
                start: r.start_offset(),
                end: r.end_offset(),
            },
            Range::Text(_) => {
                return Err(kg::QueryError::new(
                    kg::QueryErrorKind::ViewMismatch,
                    "scope",
                    "invalid text fragment coordinates",
                ));
            }
        };
        span.slice(body)?;
        fragments.push(kg::ScopeTextFragment {
            id: fragment.id.as_str().into(),
            span,
        });
    }
    Ok(kg::ScopeTextEntity {
        id: entity.id.as_str().into(),
        content_bytes: body.len() as u64,
        fragments,
    })
}

// Also used by synchronous authorization probes. The monitor is joined before
// native resources leave the worker, including early errors and cancellation.
struct NativeWatch {
    stop: mpsc::Sender<()>,
    thread: Option<std::thread::JoinHandle<()>>,
}
impl NativeWatch {
    fn new(
        ctx: &kg::RequestContext,
        signal: tokio_util::sync::CancellationToken,
        control: QueryControl,
    ) -> EngineResult<Self> {
        let (stop, receive) = mpsc::channel();
        let context = ctx.clone();
        let thread = std::thread::Builder::new()
            .name("graph-scope-cancel".into())
            .spawn(move || {
                loop {
                    if signal.is_cancelled() {
                        control.request(QueryStopReason::Cancelled);
                        break;
                    }
                    if let Err(error) = context.check() {
                        control.request(if error.kind == kg::QueryErrorKind::Cancelled {
                            QueryStopReason::Cancelled
                        } else {
                            QueryStopReason::Deadline
                        });
                        break;
                    }
                    match receive.recv_timeout(Duration::from_millis(5)) {
                        Err(mpsc::RecvTimeoutError::Timeout) => {}
                        _ => break,
                    }
                }
            })
            .map_err(|e| EngineError::from_io("start scope cancellation monitor", &e))?;
        Ok(Self {
            stop,
            thread: Some(thread),
        })
    }
}
impl Drop for NativeWatch {
    fn drop(&mut self) {
        let _ = self.stop.send(());
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn prepare(
    read: ReadSessionLease,
    manifest: WorkspaceManifest,
    admission: Arc<FileLock>,
    options: &ContextOptions,
    ctx: &kg::RequestContext,
    signal: tokio_util::sync::CancellationToken,
    include_evidence: bool,
) -> EngineResult<PreparedScope> {
    let GraphQueryMode::Constrain { scope } = &options.graph else {
        return Err(EngineError::internal("expected constrain"));
    };
    let control = QueryControl::new().map_err(|e| {
        EngineError::storage_failure(format!("scope cancellation unavailable: {e}"))
    })?;
    let _watch = NativeWatch::new(ctx, signal.clone(), control.clone())?;
    let selected = qualify(
        read,
        manifest,
        admission,
        ctx,
        signal,
        control,
        kg::QueryDomain::Code,
    )?;
    let lease = &selected.provider.lease;
    let text = Text {
        store: lease.read.storage(),
        bytes: std::cell::Cell::new(0),
        entities: std::cell::RefCell::new(BTreeMap::new()),
    };
    let mut resolution =
        kg::resolve_scope(&lease.reader, &text, &selected.view, scope, ctx).map_err(query_error)?;
    resolution.execution.qualification_micros = selected.qualification_micros;
    let entities = text.entities.into_inner();
    let files = lease.read.storage().list_files()?;
    let allowed = resolve_filtered_file_ids(
        &lease.root,
        &SearchPlan {
            routes: vec![],
            limit: None,
            trace: false,
            prefer_symbol: false,
            filter: options.filter.clone(),
        },
        &files,
    )?;
    let mut units = Vec::new();
    for unit in resolution.scope.units() {
        let entity = entities
            .get(&unit.entity_id)
            .ok_or_else(|| EngineError::storage_failure("scope entity missing"))?;
        if allowed.contains(&entity.file_id)
            && (options.filter.symbol_types.is_empty()
                || matches!(&entity.metadata,Some(EntityMetadata::Code(code)) if code.symbol_type.is_some_and(|t|options.filter.symbol_types.contains(&t))))
        {
            if unit.source_spans.len() != 1 {
                return Err(EngineError::unsupported(
                    "scope_not_representable: noncontiguous display range",
                ));
            }
            units.push(unit.clone());
        }
    }
    ctx.check().map_err(query_error)?;
    validate_scope_filter(lease, &units, options, ctx)?;
    let evidence = if include_evidence {
        call_evidence(
            &selected.view,
            &lease.reader,
            &resolution,
            &units,
            &entities,
            ctx,
        )?
    } else {
        Vec::new()
    };
    Ok(PreparedScope {
        resolution,
        units,
        entities,
        evidence,
    })
}

fn call_evidence(
    view: &kg::KgReadView<Arc<super::Lease>>,
    reader: &zg_knowledge::graph::GraphDbStore,
    resolution: &kg::ScopeResolution,
    units: &[kg::ScopeUnit],
    entities: &BTreeMap<String, Entity>,
    ctx: &kg::RequestContext,
) -> EngineResult<Vec<crate::api::context::graph_result::GraphEvidence>> {
    let mut refs = BTreeMap::new();
    for reference in resolution
        .relations
        .iter()
        .filter(|r| r.kind == kg::RelationKind::Calls)
        .filter_map(|r| r.reference.as_ref())
    {
        for unit in units {
            if unit.source_id != reference.evidence.source_id {
                continue;
            }
            let span = unit.source_spans[0];
            if span.start > reference.evidence.span.start || span.end < reference.evidence.span.end
            {
                continue;
            }
            let Content::Text(body) = &entities[&unit.entity_id].content else {
                continue;
            };
            let call_start = unit.content_span.start + reference.evidence.span.start - span.start;
            let call_end = unit.content_span.start + reference.evidence.span.end - span.start;
            let window = call_window(body, unit.content_span, call_start, call_end)?;
            let mut evidence = reference.evidence.clone();
            evidence.span = kg::ByteSpan {
                start: span.start + window.start - unit.content_span.start,
                end: span.start + window.end - unit.content_span.start,
            };
            evidence.reference_id = None;
            refs.insert(
                (
                    evidence.source_id.clone(),
                    evidence.span.start,
                    evidence.span.end,
                ),
                evidence,
            );
        }
    }
    super::evidence::HostEvidence { reader }
        .read_evidence_with_entities(view, &refs.into_values().collect::<Vec<_>>(), ctx, entities)
        .map_err(query_error)?
        .into_iter()
        .map(|read| match read {
            kg::EvidenceRead::Verified { reference, content } => {
                Ok(crate::api::context::graph_result::GraphEvidence::Verified {
                    reference,
                    content,
                })
            }
            kg::EvidenceRead::Unavailable { .. } => Err(EngineError::resource_busy(
                "scope call evidence unavailable",
            )),
        })
        .collect()
}

fn call_window(body: &str, unit: kg::ByteSpan, start: u64, end: u64) -> EngineResult<kg::ByteSpan> {
    let before = kg::ByteSpan {
        start: unit.start,
        end: start,
    }
    .slice(body)
    .map_err(query_error)?;
    let after = kg::ByteSpan {
        start: end,
        end: unit.end,
    }
    .slice(body)
    .map_err(query_error)?;
    let begin = before
        .rmatch_indices('\n')
        .nth(2)
        .map_or(unit.start, |(i, _)| unit.start + i as u64 + 1);
    let finish = after
        .match_indices('\n')
        .nth(2)
        .map_or(unit.end, |(i, _)| end + i as u64 + 1);
    Ok(kg::ByteSpan {
        start: begin,
        end: finish,
    })
}

fn validate_scope_filter(
    lease: &super::Lease,
    units: &[kg::ScopeUnit],
    options: &ContextOptions,
    ctx: &kg::RequestContext,
) -> EngineResult<()> {
    if !units.is_empty() {
        let mut filter = crate::pipelines::indexed_search::pipeline::search_plan_to_storage_filter(
            &lease.root,
            &SearchPlan {
                routes: vec![],
                limit: None,
                trace: false,
                prefer_symbol: false,
                filter: options.filter.clone(),
            },
            lease.read.storage(),
        )?
        .unwrap_or_default();
        filter.fragment_ids = Some(units.iter().map(|u| u.fragment_id.clone()).collect());
        ctx.charge(kg::BudgetUsage {
            filter_bytes: lease.read.storage().search_filter_bytes(Some(&filter))? as u64,
            ..Default::default()
        })
        .map_err(query_error)?;
    }
    Ok(())
}

impl GraphExecution {
    fn scope_context(&self) -> EngineResult<kg::RequestContext> {
        let started = *self.scope_started.get_or_init(Instant::now);
        let remaining = Duration::from_millis(self.execution.graph_timeout_ms)
            .saturating_sub(started.elapsed());
        if remaining.is_zero() {
            return Err(EngineError::deadline_exceeded(
                "scope graph stage deadline exceeded",
            ));
        }
        self.context.child(remaining).map_err(query_error)
    }
    pub(in crate::pipelines) async fn has_scope(
        &self,
        options: ContextOptions,
    ) -> EngineResult<bool> {
        let ctx = self.scope_context()?;
        let result = tokio::task::spawn_blocking(move || {
            probe(&options, &ctx).map(|scope| !scope.units.is_empty())
        })
        .await
        .map_err(|_| EngineError::internal("scope preflight worker failed"))?;
        self.check()?;
        result
    }
    pub(in crate::pipelines) async fn resolve(
        &self,
        read: ReadSessionLease,
        manifest: WorkspaceManifest,
        admission: Arc<FileLock>,
        options: ContextOptions,
    ) -> EngineResult<PreparedScope> {
        self.check()?;
        let ctx = self.scope_context()?;
        let signal = self.work_signal.clone();
        let result = tokio::task::spawn_blocking(move || {
            prepare(read, manifest, admission, &options, &ctx, signal, true)
        })
        .await
        .map_err(|_| EngineError::internal("scope worker failed"))?;
        self.check()?;
        result
    }
    pub(in crate::pipelines) fn scoped_storage<'a>(
        &self,
        read: &'a ReadSessionLease,
        scope: &'a PreparedScope,
    ) -> ScopedStorage<'a> {
        ScopedStorage::new(read.storage(), scope, self.context.clone())
    }
    pub(in crate::pipelines) fn finish_scope(
        &self,
        scope: PreparedScope,
        result: &mut ContextResult,
    ) -> EngineResult<()> {
        scope.verify_output(result)?;
        let empty_reason = if scope.resolution.nodes.is_empty() {
            Some("no_graph_nodes")
        } else if scope.resolution.scope.is_empty() {
            Some("no_searchable_text")
        } else if scope.units.is_empty() {
            Some("user_filter_empty")
        } else {
            None
        };
        let count = scope.units.len();
        let mut graph = GraphSupplement {
            scope: Some(GraphScope {
                anchor: scope.resolution.location.clone(),
                complete: true,
                exact_filter: true,
                recall_exhaustive: false,
                region_count: scope.resolution.scope.regions().len(),
                fragment_count: scope.resolution.scope.units().len(),
                filtered_fragment_count: count,
                empty_reason: empty_reason.map(str::to_owned),
            }),
            status: GraphStatus::Complete,
            view: Some(scope.resolution.scope.view().clone()),
            seeds: vec![],
            skipped: vec![],
            expansions: vec![],
            nodes: scope.resolution.nodes,
            relations: scope.resolution.relations,
            evidence: scope.evidence,
            diagnostics: vec![],
            execution: Some(scope.resolution.execution),
        };
        super::augmentation::result::account(&mut graph, &self.context)?;
        result.graph = Some(graph);
        self.check()
    }
}

/// No lost-lease scope is reused for execution; this probe only suppresses an
/// unnecessary consent prompt. Execution reselects and resolves under its lease.
pub(crate) fn inspect_scope(
    options: &ContextOptions,
) -> EngineResult<crate::authorization::ConstrainPreflight> {
    let execution: kg::ExecutionOptions = options.graph_execution.unwrap_or_default();
    let signal = options.signal.clone().unwrap_or_default();
    let ctx = kg::RequestContext::new(
        execution.budget,
        signal.clone(),
        Instant::now() + Duration::from_millis(execution.request_timeout_ms),
    )
    .map_err(query_error)?;
    let started = Instant::now();
    let scope = probe(
        options,
        &ctx.child(Duration::from_millis(execution.graph_timeout_ms))
            .map_err(query_error)?,
    )?;
    Ok(crate::authorization::ConstrainPreflight {
        empty: scope.units.is_empty(),
        usage: ctx.usage().map_err(query_error)?,
        elapsed_micros: started.elapsed().as_micros().try_into().unwrap_or(u64::MAX),
        qualification_micros: scope.resolution.execution.qualification_micros,
        query_micros: scope.resolution.execution.query_micros,
        output_limit: execution.budget.output_bytes,
    })
}
fn probe(options: &ContextOptions, ctx: &kg::RequestContext) -> EngineResult<PreparedScope> {
    let signal = options.signal.clone().unwrap_or_default();
    let root = std::path::absolute(
        options
            .root
            .as_deref()
            .unwrap_or_else(|| std::path::Path::new(".")),
    )
    .map_err(|e| EngineError::from_io("resolve scope root", &e))?;
    let location = find_nearest_workspace(&root)?
        .ok_or_else(|| EngineError::not_found("scope workspace missing"))?;
    let lock_deadline = Instant::now()
        .checked_add(Duration::from_millis(
            options.lock_timeout_ms.unwrap_or(30_000),
        ))
        .ok_or_else(|| EngineError::invalid_argument("scope lock timeout is too large"))?
        .min(ctx.deadline());
    let admission = loop {
        ctx.check().map_err(query_error)?;
        if let Some(lock) = try_home_read(&location.home, "scope authorization")? {
            break Arc::new(lock);
        }
        if Instant::now() >= lock_deadline {
            return Err(EngineError::resource_busy("scope admission timed out"));
        }
        std::thread::sleep(Duration::from_millis(5));
    };
    let manifest = read_workspace_manifest(&location.home)?
        .ok_or_else(|| EngineError::not_found("scope manifest missing"))?;
    let read = ReadSessionLease::open_graph(&manifest)?;
    let expected = text_proof(&read, &manifest)?;
    let scope = prepare(
        read.fork(),
        manifest.clone(),
        admission.clone(),
        options,
        ctx,
        signal,
        false,
    )?;
    if text_proof(&read, &manifest)? != expected {
        return Err(EngineError::storage_failure("scope text view changed"));
    }
    read.close()?;
    ctx.check().map_err(query_error)?;
    Ok(scope)
}

#[cfg(test)]
mod call_context_tests {
    use super::*;
    #[test]
    fn call_context_preserves_utf8_crlf_and_scope_boundaries() {
        let text = "outside\n中\r\na\r\ncall(\r\n value);\r\nnext();\r\noutside";
        let start = text.find("call").expect("call") as u64;
        let unit = kg::ByteSpan {
            start: 8,
            end: text.rfind("outside").expect("end") as u64,
        };
        let window = call_window(text, unit, start, start + 4).expect("window");
        assert!(window.start >= unit.start && window.end <= unit.end);
        let body = window.slice(text).expect("slice");
        assert!(body.contains("call(\r\n value);\r\nnext();"));
        assert!(!body.contains("outside"));
    }
}
