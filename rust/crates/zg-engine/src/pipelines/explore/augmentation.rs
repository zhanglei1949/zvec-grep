//! Optional graph work shares the text request's lease, deadline and budget.
use super::{
    Arc, Duration, EngineError, EngineResult, FileLock, Instant, QueryControl, QueryStopReason,
    ReadSessionLease, Watch, WorkspaceManifest, evidence, kg, qualify, query_error,
    read_workspace_manifest,
};
mod mapping;
pub(super) mod result;
use crate::{
    api::context::{
        ContextOptions, ContextResult,
        graph::GraphQueryMode,
        graph_result::{GraphSkippedHit, GraphSupplement},
    },
    pipelines::indexed_search::retained::RetainedHit,
};
use mapping::map_hits;
use result::supplement;

pub(in crate::pipelines) struct GraphExecution {
    pub(super) context: kg::RequestContext,
    pub(super) execution: kg::ExecutionOptions,
    pub(super) work_signal: tokio_util::sync::CancellationToken,
    _watch: Watch,
    pub(super) scope_started: std::sync::OnceLock<Instant>,
}

pub(in crate::pipelines) struct AugmentInput {
    pub read: ReadSessionLease,
    pub manifest: WorkspaceManifest,
    pub admission: Arc<FileLock>,
    pub hits: Vec<RetainedHit>,
    pub mode: GraphQueryMode,
    pub result: ContextResult,
    pub expected_text: String,
    pub unavailable: Option<String>,
}

impl GraphExecution {
    pub(in crate::pipelines) fn new(options: &ContextOptions) -> EngineResult<Self> {
        let execution: kg::ExecutionOptions = convert(options.graph_execution.unwrap_or_default())?;
        execution
            .validate_within(kg::ExecutionOptions::default())
            .map_err(query_error)?;
        let signal = options.signal.clone().unwrap_or_default();
        let context = kg::RequestContext::new(
            execution.budget,
            signal.clone(),
            Instant::now() + Duration::from_millis(execution.request_timeout_ms),
        )
        .map_err(query_error)?;
        let work_signal = signal.child_token();
        let worker = work_signal.clone();
        let deadline = context.deadline();
        let watch = Watch(tokio::spawn(async move {
            tokio::select! {biased;()=signal.cancelled()=>worker.cancel(),()=tokio::time::sleep_until(tokio::time::Instant::from_std(deadline))=>worker.cancel()}
        }));
        Ok(Self {
            context,
            execution,
            work_signal,
            _watch: watch,
            scope_started: std::sync::OnceLock::new(),
        })
    }

    pub(in crate::pipelines) fn options(&self, options: &ContextOptions) -> ContextOptions {
        ContextOptions {
            signal: Some(self.work_signal.clone()),
            ..options.clone()
        }
    }

    pub(in crate::pipelines) fn check(&self) -> EngineResult<()> {
        self.context.check().map_err(query_error)
    }

    pub(in crate::pipelines) async fn finish(
        &self,
        input: AugmentInput,
    ) -> EngineResult<ContextResult> {
        self.check()?;
        let started = Instant::now();
        let context = self
            .context
            .child(Duration::from_millis(self.execution.graph_timeout_ms))
            .map_err(query_error)?;
        let control = QueryControl::new().map_err(|e| {
            EngineError::storage_failure(format!("graph cancellation unavailable: {e}"))
        })?;
        let interrupt = control.clone();
        let deadline = context.deadline();
        let signal = self.work_signal.clone();
        let cancellation = signal.clone();
        let watch = Watch(tokio::spawn(async move {
            tokio::select! {biased;()=cancellation.cancelled()=>interrupt.request(QueryStopReason::Cancelled),()=tokio::time::sleep_until(tokio::time::Instant::from_std(deadline))=>interrupt.request(QueryStopReason::Deadline)}
        }));
        let outcome = tokio::task::spawn_blocking(move || {
            let _watch = watch;
            let graph = if let Some(reason) = &input.unavailable {
                Ok(GraphSupplement::degraded(reason.clone()))
            } else {
                execute(&input, &context, signal, control)
            };
            let safe = text_proof(&input.read, &input.manifest).and_then(|actual| {
                if actual == input.expected_text {
                    Ok(())
                } else {
                    Err(EngineError::storage_failure(
                        "text view changed during retrieval",
                    ))
                }
            });
            for hit in &input.hits {
                hit.verify(input.read.storage())?;
            }
            let close = input.read.close();
            safe?;
            close?;
            Ok::<_, EngineError>((input.result, graph))
        })
        .await
        .map_err(|_| EngineError::internal("augmentation worker failed"))?;
        self.check()?;
        let (mut result, graph) = outcome?;
        result.graph = Some(match graph {
            Ok(graph) => graph,
            Err(error)
                if matches!(
                    error.code(),
                    EngineError::RESOURCE_BUSY | EngineError::DEADLINE_EXCEEDED
                ) =>
            {
                GraphSupplement::degraded(error.message().to_owned())
            }
            Err(error) => return Err(error),
        });
        result
            .diagnostics
            .timings
            .push(crate::api::context::result::TimingEntry {
                name: "graph_stage".into(),
                duration_micros: started.elapsed().as_micros().try_into().unwrap_or(u64::MAX),
                count: None,
            });
        Ok(result)
    }
}

pub(in crate::pipelines) fn text_proof(
    read: &ReadSessionLease,
    manifest: &WorkspaceManifest,
) -> EngineResult<String> {
    use crate::storage::graph_state::{self, TextPhase, TextState};
    let active = read_workspace_manifest(&manifest.path)?
        .ok_or_else(|| EngineError::storage_failure("text manifest missing"))?;
    if active.storage_generation != manifest.storage_generation {
        return Err(EngineError::storage_failure("text generation changed"));
    }
    let files = read.storage().list_files()?;
    let digest = graph_state::catalog_digest(&files);
    let state = TextState::read(&manifest.storage_home())?;
    if state
        .as_ref()
        .is_some_and(|s| s.state != TextPhase::Stable || s.catalog_digest != digest)
    {
        return Err(EngineError::storage_failure(
            "text view is unstable or its catalog changed",
        ));
    }
    graph_state::fingerprint(&(manifest.storage_generation.as_deref(), digest, state))
}

impl Drop for GraphExecution {
    fn drop(&mut self) {
        self.work_signal.cancel();
    }
}

fn convert<T: serde::Serialize, U: serde::de::DeserializeOwned>(value: T) -> EngineResult<U> {
    serde_json::from_value(
        serde_json::to_value(value)
            .map_err(|_| EngineError::internal("encode graph query value"))?,
    )
    .map_err(|_| EngineError::internal("convert graph query value"))
}

fn execute(
    input: &AugmentInput,
    context: &kg::RequestContext,
    signal: tokio_util::sync::CancellationToken,
    control: QueryControl,
) -> EngineResult<GraphSupplement> {
    let selected = qualify(
        input.read.fork(),
        input.manifest.clone(),
        input.admission.clone(),
        context,
        signal,
        control,
    )?;
    let reader = &selected.provider.lease.reader;
    let query_started = Instant::now();
    let (hits, skipped) = map_hits(&selected, &input.hits, context)?;
    let evidence = evidence::HostEvidence { reader };
    let mut augmented = kg::augment(
        reader,
        &evidence,
        &selected.view,
        &hits,
        &input.mode.clone().try_into()?,
        context,
    )
    .map_err(query_error)?;
    augmented.execution.qualification_micros = selected.qualification_micros;
    augmented.execution.query_micros = query_started
        .elapsed()
        .as_micros()
        .try_into()
        .unwrap_or(u64::MAX);
    let mut graph = supplement(augmented, skipped, &input.result)?;
    result::account(&mut graph, context)?;
    context.check().map_err(query_error)?;
    Ok(graph)
}
