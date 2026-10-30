//! Pure read admission and one bounded native worker. No indexing or model calls.
pub(in crate::pipelines) mod augmentation;
pub(crate) mod constrain;
mod evidence;
use crate::{
    EngineError, EngineResult,
    api::explore::{ExploreOptions, ExploreResult},
    storage::read_session::{ReadSessionCache, ReadSessionLease},
    workspace::{
        layout::find_nearest_workspace,
        lock::{FileLock, LockWait, try_home_read},
        manifest::{WorkspaceManifest, read_workspace_manifest},
    },
};
use std::{
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};
use zg_knowledge::{
    graph::{GraphLimits, QueryControl, QueryStopReason},
    processing::ProcessingContext,
    query as kg,
};

pub(super) fn query_error(error: kg::QueryError) -> EngineError {
    use kg::QueryErrorKind as K;
    let message = format!(
        "{:?} at {}: {}{}",
        error.kind,
        error.stage,
        error.reason,
        error.detail.map_or_else(String::new, |d| format!("; {d}"))
    );
    match error.kind {
        K::InvalidInput => EngineError::invalid_argument(message),
        K::Unsupported => EngineError::unsupported(message),
        K::Cancelled => EngineError::cancelled(message),
        K::RequestDeadline | K::StageDeadline => EngineError::deadline_exceeded(message),
        K::GraphUnavailable | K::ViewMismatch | K::ResourceExhausted => {
            EngineError::resource_busy(message)
        }
        K::RecoveryRequired | K::Internal => EngineError::storage_failure(message),
    }
}
struct Lease {
    // Field order keeps the admission lock until both native facades are gone.
    reader: zg_knowledge::graph::GraphDbStore,
    read: ReadSessionLease,
    manifest: WorkspaceManifest,
    root: PathBuf,
    _admission: Arc<FileLock>,
}
struct Provider {
    lease: Arc<Lease>,
    stamp: kg::ViewStamp,
}
impl kg::ReadViewProvider for Provider {
    type Lease = Arc<Lease>;
    fn open_view(
        &self,
        request: &kg::ReadViewRequest,
        context: &kg::RequestContext,
    ) -> kg::QueryResult<kg::KgReadView<Self::Lease>> {
        context.check()?;
        kg::KgReadView::new(
            self.stamp.clone(),
            Arc::clone(&self.lease),
            request,
            vec![kg::GraphReadCapabilities {
                domain: kg::QueryDomain::Code,
                relations: vec![
                    kg::RelationKind::Contains,
                    kg::RelationKind::Calls,
                    kg::RelationKind::Imports,
                ],
                text_mapping: true,
            }],
            None,
        )
    }
}
struct Watch(tokio::task::JoinHandle<()>);
impl Drop for Watch {
    fn drop(&mut self) {
        self.0.abort();
    }
}

pub(crate) async fn run(
    options: ExploreOptions,
    cache: Option<ReadSessionCache>,
) -> EngineResult<ExploreResult> {
    options.validate()?;
    let request: kg::ExploreRequest = serde_json::from_value(
        serde_json::to_value(&options.request)
            .map_err(|_| EngineError::invalid_argument("invalid explore request"))?,
    )
    .map_err(|_| EngineError::invalid_argument("invalid explore request"))?;
    request.validate().map_err(query_error)?;
    let execution: kg::ExecutionOptions = serde_json::from_value(
        serde_json::to_value(options.execution)
            .map_err(|_| EngineError::invalid_argument("invalid explore execution"))?,
    )
    .map_err(|_| EngineError::invalid_argument("invalid explore execution"))?;
    execution
        .validate_within(kg::ExecutionOptions::default())
        .map_err(query_error)?;
    let signal = options.signal.clone().unwrap_or_default().child_token();
    let _cancel_on_drop = signal.clone().drop_guard();
    let context = kg::RequestContext::new(
        execution.budget,
        signal.clone(),
        Instant::now() + Duration::from_millis(execution.request_timeout_ms),
    )
    .map_err(query_error)?;
    context.check().map_err(query_error)?;
    let root = std::path::absolute(
        options
            .root
            .as_deref()
            .unwrap_or_else(|| std::path::Path::new(".")),
    )
    .map_err(|e| EngineError::from_io("resolve explore root", &e))?;
    let location = find_nearest_workspace(&root)?.ok_or_else(|| {
        EngineError::not_found("workspace index missing; build the index explicitly")
    })?;
    let wait = LockWait::new(
        Some(&signal),
        Some(
            options
                .lock_timeout_ms
                .unwrap_or(execution.request_timeout_ms)
                .min(execution.request_timeout_ms),
        ),
    )?;
    let admission = loop {
        context.check().map_err(query_error)?;
        if let Some(lock) = try_home_read(&location.home, "explore")? {
            break Arc::new(lock);
        }
        wait.retry(&location.home, "explore").await?;
    };
    let context = context
        .child(Duration::from_millis(execution.graph_timeout_ms))
        .map_err(query_error)?;
    let control = QueryControl::new().map_err(|e| {
        EngineError::storage_failure(format!("graph cancellation unavailable: {e}"))
    })?;
    let cancellation = signal.clone();
    let deadline = context.deadline();
    let interrupt = control.clone();
    let watch = Watch(tokio::spawn(async move {
        tokio::select! {biased;()=cancellation.cancelled()=>interrupt.request(QueryStopReason::Cancelled),()=tokio::time::sleep_until(tokio::time::Instant::from_std(deadline))=>interrupt.request(QueryStopReason::Deadline)}
    }));
    tokio::task::spawn_blocking(move || {
        let _watch = watch;
        context.check().map_err(query_error)?;
        execute_selected(
            &location.home,
            cache,
            admission,
            &context,
            signal,
            control,
            &request,
        )
    })
    .await
    .map_err(|_| EngineError::internal("explore worker failed"))?
}

fn execute_selected(
    home: &std::path::Path,
    cache: Option<ReadSessionCache>,
    admission: Arc<FileLock>,
    context: &kg::RequestContext,
    signal: tokio_util::sync::CancellationToken,
    control: QueryControl,
    request: &kg::ExploreRequest,
) -> EngineResult<ExploreResult> {
    let _profile = zg_knowledge::graph::profile::BuildProfile::start("explore");
    let open_stage = zg_knowledge::graph::profile::stage("qualification.open");
    let manifest = read_workspace_manifest(home)?
        .ok_or_else(|| EngineError::not_found("workspace manifest missing"))?;
    let read = match cache {
        Some(cache) => cache.acquire_graph_query(&manifest)?,
        None => ReadSessionLease::open_graph(&manifest)?,
    };
    drop(open_stage);
    let selected = qualify(read, manifest, admission, context, signal, control)?;
    request
        .validate_for(selected.view.stamp())
        .map_err(query_error)?;
    let reader = &selected.provider.lease.reader;
    let evidence = evidence::HostEvidence { reader };
    let _business = zg_knowledge::graph::profile::stage("explore.business");
    let mut result =
        kg::explore(reader, &evidence, &selected.view, request, context).map_err(query_error)?;
    result.execution.qualification_micros = selected.qualification_micros;
    context.check().map_err(query_error)?;
    serde_json::from_value(
        serde_json::to_value(result).map_err(|_| EngineError::internal("encode explore result"))?,
    )
    .map_err(|_| EngineError::internal("convert explore result"))
}

/// Only this constructor turns a selected descriptor into a verified view.
/// The text lease was acquired before retrieval and remains owned by the view.
fn qualify(
    read: ReadSessionLease,
    manifest: WorkspaceManifest,
    admission: Arc<FileLock>,
    context: &kg::RequestContext,
    signal: tokio_util::sync::CancellationToken,
    control: QueryControl,
) -> EngineResult<QualifiedView> {
    use kg::ReadViewProvider;
    let started = Instant::now();
    let limits = GraphLimits {
        max_rows: 4_000_000,
        control: Some(control),
        ..Default::default()
    };
    let processing = ProcessingContext {
        max_input_bytes: 256 << 20,
        max_output_items: 30_000_000,
        deadline: Some(context.deadline()),
        cancellation: signal,
    };
    read.validate_graph(&manifest, limits.clone(), &processing)?;
    let stamp = read.graph_stamp(&manifest)?;
    let provider = Provider {
        stamp,
        lease: Arc::new(Lease {
            reader: read.graph_reader(limits)?,
            read,
            root: manifest.workspace.root.clone(),
            manifest,
            _admission: admission,
        }),
    };
    let view = provider
        .open_view(
            &kg::ReadViewRequest {
                graph: kg::GraphRequirement::Required,
                needs: Some(kg::GraphReadNeeds {
                    domain: kg::QueryDomain::Code,
                    relations: vec![],
                    text_mapping: false,
                }),
            },
            context,
        )
        .map_err(query_error)?;
    Ok(QualifiedView {
        provider,
        view,
        qualification_micros: started.elapsed().as_micros().try_into().unwrap_or(u64::MAX),
    })
}

struct QualifiedView {
    provider: Provider,
    view: kg::KgReadView<Arc<Lease>>,
    qualification_micros: u64,
}
