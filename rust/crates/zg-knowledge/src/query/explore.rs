//! Deterministic business traversal. All reads share one view and budget.
mod augment;
mod paging;
mod scope;
use super::{
    BudgetUsage, Completion, CoverageDiagnostic, Direction, EntityRecord, EvidenceProvider,
    EvidenceRead, EvidenceRef, ExploreOperation, ExploreRequest, ExploreResult, GraphNode,
    GraphPath, GraphReadNeeds, GraphReader, KgReadView, LocateResult, LocateStatus, Locator,
    NodeRef, QueryDomain, QueryError, QueryErrorKind, QueryExecution, QueryResult,
    ReferencePartition, ReferenceRecord, ReferenceSelector, RelationKind, RelationRecord,
    RequestContext, SourceFilter, TraversalSpec, ViewStamp,
};
pub use augment::{AugmentResult, SeedExpansion, augment};
pub use scope::{
    ScopeRegion, ScopeResolution, ScopeTextEntity, ScopeTextFragment, ScopeTextProvider, ScopeUnit,
    resolve_scope, resolve_semantic_scope,
};
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    time::Instant,
};

/// Reader page size shared by locate, neighbor and reference pagination.
const PAGE: u16 = 256;

fn permits(filters: &[&SourceFilter], source: &str) -> bool {
    filters.iter().all(|f| {
        f.sources
            .as_ref()
            .is_none_or(|s| s.iter().any(|s| s == source))
    })
}
fn permits_node(filters: &[&SourceFilter], node: &EntityRecord) -> bool {
    permits(filters, &node.source_id)
        && filters.iter().all(|filter| {
            filter.kinds.as_ref().is_none_or(|kinds| {
                serde_json::to_value(node.kind)
                    .ok()
                    .and_then(|v| v.as_str().map(|v| kinds.iter().any(|k| k == v)))
                    .unwrap_or(false)
            })
        })
}
fn node(view: &ViewStamp, r: &EntityRecord, distance: Option<u32>) -> QueryResult<GraphNode> {
    Ok(GraphNode {
        node: NodeRef {
            generation: view.generation.clone(),
            workspace_id: view.workspace_id.clone(),
            graph_build_id: view.require_graph()?.build_id.clone(),
            node_id: r.id.clone(),
        },
        source_id: r.source_id.clone(),
        kind: r.kind,
        name: r.name.clone(),
        qualified_name: r.qualified_name.clone(),
        span: r.span,
        distance,
    })
}
struct Search<'a, R> {
    reader: &'a R,
    view: &'a ViewStamp,
    ctx: &'a RequestContext,
    nodes: BTreeMap<String, GraphNode>,
    relations: BTreeMap<String, RelationRecord>,
    anchor_ready: bool,
    locations: Vec<LocateResult>,
    references: Vec<ReferenceRecord>,
    paths: Vec<GraphPath>,
}
impl<R: GraphReader> Search<'_, R> {
    fn locate(
        &mut self,
        locator: &Locator,
        boundary: &[&SourceFilter],
    ) -> QueryResult<Option<EntityRecord>> {
        let mut candidates = Vec::new();
        let mut offset = 0;
        loop {
            self.ctx.check()?;
            let page = self.reader.locate(locator, offset, self.ctx)?;
            self.ctx.charge(BudgetUsage {
                nodes: page.len() as u64,
                candidates: page.len() as u64,
                ..Default::default()
            })?;
            let count = page.len();
            candidates.extend(page.into_iter().filter(|n| permits(boundary, &n.source_id)));
            if count < usize::from(PAGE) {
                break;
            }
            offset += u64::from(PAGE);
        }
        if matches!(locator, Locator::Position { .. })
            && let Some(size) = candidates.iter().map(|n| n.span.end - n.span.start).min()
        {
            candidates.retain(|n| n.span.end - n.span.start == size);
        }
        let status = match candidates.len() {
            0 => LocateStatus::NotFound,
            1 => LocateStatus::Unique,
            _ => LocateStatus::Ambiguous,
        };
        self.anchor_ready = status == LocateStatus::Unique;
        self.locations.push(LocateResult {
            locator: locator.clone(),
            status,
            candidates: candidates
                .iter()
                .map(|r| node(self.view, r, None))
                .collect::<QueryResult<_>>()?,
        });
        Ok(if candidates.len() == 1 {
            candidates.pop()
        } else {
            None
        })
    }
    fn get_nodes(&self, ids: &[String]) -> QueryResult<Vec<EntityRecord>> {
        let mut out = Vec::new();
        for batch in ids.chunks(usize::from(PAGE)) {
            let page = self.reader.get_nodes(batch, self.ctx)?;
            self.ctx.charge(BudgetUsage {
                nodes: page.len() as u64,
                ..Default::default()
            })?;
            if page.len() != batch.len() {
                return Err(QueryError::new(
                    QueryErrorKind::ViewMismatch,
                    "nodes",
                    "missing graph endpoint",
                ));
            }
            out.extend(page);
        }
        Ok(out)
    }
    fn edges(
        &self,
        frontier: &[String],
        kinds: &[RelationKind],
        direction: Direction,
        boundary: &[&SourceFilter],
    ) -> QueryResult<Vec<RelationRecord>> {
        let mut edges = BTreeMap::new();
        let mut allowed_sources: Option<Vec<String>> = None;
        let mut allowed_kinds: Option<Vec<String>> = None;
        for filter in boundary {
            for (value, next) in [
                (&mut allowed_sources, &filter.sources),
                (&mut allowed_kinds, &filter.kinds),
            ] {
                if let Some(next) = next {
                    match value {
                        Some(current) => current.retain(|v| next.contains(v)),
                        None => *value = Some(next.clone()),
                    }
                }
            }
        }
        let merged = SourceFilter {
            sources: allowed_sources,
            kinds: allowed_kinds,
        };
        for anchor in frontier {
            for kind in kinds {
                let mut after = None;
                loop {
                    let identity = crate::semantic::ObjectId::SourceNode {
                        node_id: anchor.clone(),
                    };
                    let selector = crate::query::RelationSelector {
                        edge_type: *kind,
                        relation_types: None,
                    };
                    let page = self.reader.neighbors_after(
                        &crate::query::NeighborRead {
                            anchor: &identity,
                            selector: &selector,
                            direction,
                            after: after.as_ref(),
                            skip_group: false,
                            boundary: &merged,
                            contexts: None,
                            spaces: None,
                        },
                        self.ctx,
                    )?;
                    let next = page.last().map(RelationRecord::key);
                    self.ctx.charge(BudgetUsage {
                        edges: page.len() as u64,
                        ..Default::default()
                    })?;
                    let count = page.len();
                    let ids = page
                        .iter()
                        .flat_map(|e| [e.source.clone(), e.target.clone()])
                        .collect::<BTreeSet<_>>()
                        .into_iter()
                        .collect::<Vec<_>>();
                    let records = self.get_nodes(&ids)?;
                    let steps = records
                        .iter()
                        .filter(|node| permits_node(boundary, node))
                        .map(|node| node.id.as_str())
                        .collect::<BTreeSet<_>>();
                    let allowed = records
                        .iter()
                        .filter(|n| permits(boundary, &n.source_id))
                        .map(|n| n.id.clone())
                        .collect::<BTreeSet<_>>();
                    for edge in page {
                        if allowed.contains(&edge.source)
                            && allowed.contains(&edge.target)
                            && steps.contains(if edge.source == *anchor {
                                edge.target.as_str()
                            } else {
                                edge.source.as_str()
                            })
                            && edge
                                .reference
                                .as_ref()
                                .is_none_or(|r| permits(boundary, &r.evidence.source_id))
                        {
                            edges.insert(edge.id.clone(), edge);
                        }
                    }
                    if count < usize::from(PAGE) {
                        break;
                    }
                    after = next;
                }
            }
        }
        Ok(edges.into_values().collect())
    }
    fn insert(&mut self, r: &EntityRecord, distance: Option<u32>) -> QueryResult<()> {
        self.nodes
            .insert(r.id.clone(), node(self.view, r, distance)?);
        Ok(())
    }
    fn expand(
        &mut self,
        start: &EntityRecord,
        spec: &TraversalSpec,
        boundary: &[&SourceFilter],
        filter: &[&SourceFilter],
    ) -> QueryResult<()> {
        let mut visited = BTreeMap::from([(start.id.clone(), 0)]);
        let mut frontier = vec![start.id.clone()];
        if spec.include_start && permits_node(filter, start) {
            self.insert(start, Some(0))?;
        }
        for depth in 1..=spec.max_hops {
            if frontier.is_empty() {
                break;
            }
            let edges = self.edges(&frontier, &spec.relations, spec.direction, boundary)?;
            let mut next = BTreeSet::new();
            for edge in &edges {
                for id in [&edge.source, &edge.target] {
                    if !visited.contains_key(id) {
                        visited.insert(id.clone(), depth);
                        next.insert(id.clone());
                    }
                }
            }
            let records = self.get_nodes(&next.iter().cloned().collect::<Vec<_>>())?;
            for record in &records {
                if depth >= spec.min_hops && permits_node(filter, record) {
                    self.insert(record, Some(depth))?;
                }
            }
            // Retain concrete business edges connecting selected nodes and their
            // traversal predecessors; intermediate endpoints remain explicit.
            if depth >= spec.min_hops {
                for edge in edges {
                    let destination = if frontier.contains(&edge.source) {
                        &edge.target
                    } else {
                        &edge.source
                    };
                    if self.nodes.contains_key(destination) {
                        self.relations.insert(edge.id.clone(), edge);
                    }
                }
            }
            frontier = next.into_iter().collect();
        }
        Ok(())
    }
    fn path(
        &mut self,
        start: &EntityRecord,
        target: &EntityRecord,
        spec: &TraversalSpec,
        boundary: &[&SourceFilter],
        filter: &[&SourceFilter],
    ) -> QueryResult<()> {
        if !permits_node(filter, target) {
            return Ok(());
        }
        let mut queue = VecDeque::from([(vec![start.id.clone()], Vec::<RelationRecord>::new())]);
        while let Some((path, relations)) = queue.pop_front() {
            self.ctx.check()?;
            let depth = u32::try_from(relations.len())
                .map_err(|_| QueryError::invalid("path length overflow"))?;
            if depth >= spec.max_hops {
                continue;
            }
            let last = path
                .last()
                .ok_or_else(|| QueryError::invalid("empty path state"))?;
            let mut edges = self.edges(
                std::slice::from_ref(last),
                &spec.relations,
                spec.direction,
                boundary,
            )?;
            edges.sort_by(|a, b| a.id.cmp(&b.id));
            for edge in edges {
                let next = if edge.source == *last {
                    &edge.target
                } else {
                    &edge.source
                };
                if path.contains(next) {
                    continue;
                }
                let mut path = path.clone();
                path.push(next.clone());
                let mut relations = relations.clone();
                relations.push(edge.clone());
                self.ctx.charge(BudgetUsage {
                    candidates: 1,
                    ..Default::default()
                })?;
                if *next == target.id && depth + 1 >= spec.min_hops {
                    let records = self.get_nodes(&path)?;
                    for record in records {
                        let distance = path
                            .iter()
                            .position(|id| id == &record.id)
                            .and_then(|n| u32::try_from(n).ok());
                        self.insert(&record, distance)?;
                    }
                    let refs = relations.iter().map(|r| r.id.clone()).collect();
                    for r in relations {
                        self.relations.insert(r.id.clone(), r);
                    }
                    self.paths.push(GraphPath {
                        nodes: path
                            .iter()
                            .map(|id| NodeRef {
                                generation: self.view.generation.clone(),
                                workspace_id: self.view.workspace_id.clone(),
                                graph_build_id: self
                                    .view
                                    .graph
                                    .as_ref()
                                    .map(|g| g.build_id.clone())
                                    .unwrap_or_default(),
                                node_id: id.clone(),
                            })
                            .collect(),
                        relations: refs,
                    });
                    return Ok(());
                }
                queue.push_back((path, relations));
            }
        }
        Ok(())
    }
}

/// Read-only code exploration. Providers belong to the same leased view.
pub fn explore<L, R: GraphReader, E: EvidenceProvider<L>>(
    reader: &R,
    evidence: &E,
    view: &KgReadView<L>,
    request: &ExploreRequest,
    context: &RequestContext,
) -> QueryResult<ExploreResult> {
    request.validate_for(view.stamp())?;
    if matches!(
        request.operation,
        ExploreOperation::Neighbors { .. } | ExploreOperation::RelationTypes { .. }
    ) {
        return paging::explore(reader, evidence, view, request, context);
    }
    if let ExploreOperation::Semantic { query } = &request.operation {
        return super::semantic_read::explore(reader, evidence, view, request, query, context);
    }
    view.require(&read_needs(request)?)?;
    let started = Instant::now();
    let mut s = Search {
        reader,
        view: view.stamp(),
        ctx: context,
        nodes: BTreeMap::new(),
        relations: BTreeMap::new(),
        anchor_ready: false,
        locations: vec![],
        references: vec![],
        paths: vec![],
    };
    let mut partial = false;
    let result = s.execute(request);
    if let Err(error) = result {
        let list = matches!(
            request.operation,
            ExploreOperation::Neighbors { .. }
                | ExploreOperation::Expand { .. }
                | ExploreOperation::References { .. }
        );
        if error.kind == QueryErrorKind::ResourceExhausted && list && s.anchor_ready {
            partial = true;
        } else {
            return Err(error);
        }
    }
    let refs = s.evidence_refs()?;
    let evidence = evidence.read_evidence(view, &refs, context)?;
    if evidence
        .iter()
        .any(|e| matches!(e, EvidenceRead::Unavailable { .. }))
    {
        return Err(QueryError::new(
            QueryErrorKind::ViewMismatch,
            "evidence",
            "required source evidence unavailable",
        ));
    }
    let mut result = ExploreResult {
        next_cursor: None,
        relation_types: Vec::new(),
        semantic: None,
        view: view.stamp().clone(),
        locations: s.locations,
        nodes: s.nodes.into_values().collect(),
        relations: s.relations.into_values().collect(),
        paths: s.paths,
        references: s.references,
        evidence,
        coverage: CoverageDiagnostic {
            domain: QueryDomain::Code,
            ready: true,
            note: "static extraction; unresolved references reported separately".into(),
        },
        execution: QueryExecution {
            status: if partial {
                Completion::Partial
            } else {
                Completion::Complete
            },
            stop_reason: partial.then(|| "graph_budget".into()),
            usage: context.usage()?,
            qualification_micros: 0,
            query_micros: started.elapsed().as_micros().try_into().unwrap_or(u64::MAX),
        },
    };
    let list = matches!(
        request.operation,
        ExploreOperation::Neighbors { .. }
            | ExploreOperation::Expand { .. }
            | ExploreOperation::References { .. }
    );
    account_output(&mut result, context, list)?;
    Ok(result)
}

fn trim_semantic(result: &mut ExploreResult) -> QueryResult<bool> {
    if let Some(semantic) = &mut result.semantic {
        if semantic.changes.pop().is_none()
            && semantic.mentions.pop().is_none()
            && semantic.objects.pop().is_none()
        {
            return Err(QueryError::new(
                QueryErrorKind::ResourceExhausted,
                "output",
                "semantic result exceeds output budget",
            ));
        }
        let ids = semantic
            .objects
            .iter()
            .filter_map(|object| match &object.reference.object {
                crate::semantic::ObjectId::SemanticEntity { identity }
                | crate::semantic::ObjectId::Episode { identity }
                | crate::semantic::ObjectId::Fact { identity } => Some(identity),
                crate::semantic::ObjectId::SourceNode { .. } => None,
            })
            .collect::<BTreeSet<_>>();
        semantic
            .changes
            .retain(|change| ids.contains(change.target()));
        semantic.evidence.retain(|read| {
            let reference = match read {
                EvidenceRead::Verified { reference, .. }
                | EvidenceRead::Unavailable { reference, .. } => reference,
            };
            semantic
                .mentions
                .iter()
                .any(|mention| &mention.evidence == reference)
        });
        let used = semantic
            .objects
            .iter()
            .flat_map(|o| o.provenance.iter().map(|p| &p.evidence.source.source_id))
            .chain(
                semantic
                    .changes
                    .iter()
                    .map(|c| &c.evidence().source.source_id),
            )
            .chain(semantic.mentions.iter().map(|m| &m.evidence.source_id))
            .collect::<BTreeSet<_>>();
        semantic
            .sources
            .retain(|source| used.contains(&source.source.source_id));
        result.execution.status = Completion::Partial;
        result.execution.stop_reason = Some("output_budget".into());
        return Ok(true);
    }
    Ok(false)
}

pub(super) fn account_output(
    result: &mut ExploreResult,
    context: &RequestContext,
    list: bool,
) -> QueryResult<()> {
    loop {
        context.check()?;
        // Reserve the maximum width for qualification timing populated by the
        // host. Resolve the shared byte counter's decimal width arithmetically
        // after one serialization, including at a power-of-ten boundary.
        let used = context.usage()?.output_bytes;
        let available = context.limits().output_bytes.saturating_sub(used);
        let qualification = result.execution.qualification_micros;
        result.execution.qualification_micros = u64::MAX;
        result.execution.usage.output_bytes = 0;
        let base = serde_json::to_vec(result)
            .map_err(|_| QueryError::invalid("result serialization failed"))?
            .len() as u64
            - 1;
        result.execution.qualification_micros = qualification;
        result.execution.usage.output_bytes = used;
        let mut bytes = base + 1;
        while bytes <= available {
            let actual = base + u64::from((used + bytes).ilog10()) + 1;
            if actual == bytes {
                break;
            }
            bytes = actual;
        }
        if bytes <= available {
            context.charge(BudgetUsage {
                output_bytes: bytes,
                ..Default::default()
            })?;
            result.execution.usage = context.usage()?;
            return Ok(());
        }
        if !list {
            return Err(QueryError::new(
                QueryErrorKind::ResourceExhausted,
                "output",
                "complete result exceeds output budget",
            ));
        }
        if trim_semantic(result)? {
            continue;
        }
        if result.references.pop().is_none() {
            if result.relations.pop().is_none() {
                return Err(QueryError::new(
                    QueryErrorKind::ResourceExhausted,
                    "output",
                    "required location evidence exceeds output budget",
                ));
            }
            let connected = result
                .relations
                .iter()
                .flat_map(|e| [e.source.as_str(), e.target.as_str()])
                .collect::<BTreeSet<_>>();
            result
                .nodes
                .retain(|n| connected.contains(n.node.node_id.as_str()));
        }
        let mut used = BTreeSet::new();
        for r in result
            .relations
            .iter()
            .filter_map(|r| r.reference.as_ref())
            .chain(result.references.iter())
        {
            used.insert((
                r.evidence.source_id.clone(),
                r.evidence.span.start,
                r.evidence.span.end,
                r.evidence.reference_id.clone(),
            ));
        }
        for node in result
            .nodes
            .iter()
            .chain(result.locations.iter().flat_map(|l| l.candidates.iter()))
        {
            used.insert((node.source_id.clone(), node.span.start, node.span.end, None));
        }
        result.evidence.retain(|e| {
            let reference = match e {
                EvidenceRead::Verified { reference, .. }
                | EvidenceRead::Unavailable { reference, .. } => reference,
            };
            used.contains(&(
                reference.source_id.clone(),
                reference.span.start,
                reference.span.end,
                reference.reference_id.clone(),
            ))
        });
        result.execution.status = Completion::Partial;
        result.execution.stop_reason = Some("output_budget".into());
    }
}

impl<R: GraphReader> Search<'_, R> {
    fn execute(&mut self, request: &ExploreRequest) -> QueryResult<()> {
        let boundary = [&request.boundary];
        let filter = [&request.result_filter];
        match &request.operation {
            ExploreOperation::RelationTypes { .. } => {
                unreachable!("paged operations dispatch before graph search")
            }
            ExploreOperation::Semantic { .. } => {
                unreachable!("semantic dispatch precedes code search")
            }
            ExploreOperation::Locate { target } => {
                self.locate(target, &boundary)?;
            }
            ExploreOperation::Neighbors {
                anchor,
                relations,
                direction,
            } => self.neighbor_request(anchor, relations, *direction, &boundary, &filter)?,
            ExploreOperation::Expand { anchor, traversal } => {
                if let Some(start) =
                    self.locate(anchor, &[&request.boundary, &traversal.boundary])?
                {
                    self.expand(
                        &start,
                        traversal,
                        &[&request.boundary, &traversal.boundary],
                        &[&request.result_filter, &traversal.result_filter],
                    )?;
                }
            }
            ExploreOperation::ShortestPath {
                start,
                target,
                traversal,
            } => {
                let start = self.locate(start, &[&request.boundary, &traversal.boundary])?;
                let target = self.locate(target, &[&request.boundary, &traversal.boundary])?;
                if let (Some(start), Some(target)) = (start, target) {
                    self.path(
                        &start,
                        &target,
                        traversal,
                        &[&request.boundary, &traversal.boundary],
                        &[&request.result_filter, &traversal.result_filter],
                    )?;
                }
            }
            ExploreOperation::References { selector } => {
                self.reference_request(selector, &boundary, &filter)?;
            }
        }
        Ok(())
    }
    fn neighbor_request(
        &mut self,
        anchor: &Locator,
        relations: &[RelationKind],
        direction: Direction,
        boundary: &[&SourceFilter],
        filter: &[&SourceFilter],
    ) -> QueryResult<()> {
        if let Some(start) = self.locate(anchor, boundary)? {
            let edges = self.edges(
                std::slice::from_ref(&start.id),
                relations,
                direction,
                boundary,
            )?;
            for edge in edges {
                let other = if edge.source == start.id {
                    &edge.target
                } else {
                    &edge.source
                };
                let records = self.get_nodes(std::slice::from_ref(other))?;
                if let Some(record) = records.first().filter(|n| permits_node(filter, n)) {
                    self.insert(record, Some(1))?;
                    self.relations.insert(edge.id.clone(), edge);
                }
            }
        }
        Ok(())
    }
    fn reference_request(
        &mut self,
        selector: &ReferenceSelector,
        boundary: &[&SourceFilter],
        filter: &[&SourceFilter],
    ) -> QueryResult<()> {
        let mut owner = None;
        let mut source = None;
        let mut id = None;
        match selector {
            ReferenceSelector::Owner { owner: locator } => {
                let Some(found) = self.locate(locator, boundary)? else {
                    return Ok(());
                };
                owner = Some(found.id);
            }
            ReferenceSelector::Source { source_id } => {
                source = Some(source_id.clone());
                self.anchor_ready = true;
            }
            ReferenceSelector::Id { reference_id, .. } => {
                id = Some(reference_id.clone());
                self.anchor_ready = true;
            }
        }
        for partition in [
            ReferencePartition::Unresolved,
            ReferencePartition::Calls,
            ReferencePartition::Imports,
            ReferencePartition::Extends,
            ReferencePartition::Implements,
        ] {
            let mut offset = 0;
            loop {
                let page = self.reader.references(
                    partition,
                    owner.as_deref(),
                    source.as_deref(),
                    id.as_deref(),
                    offset,
                    self.ctx,
                )?;
                self.ctx.charge(BudgetUsage {
                    edges: page.len() as u64,
                    ..Default::default()
                })?;
                let count = page.len();
                self.references.extend(page.into_iter().filter(|r| {
                    permits(boundary, &r.evidence.source_id)
                        && permits(filter, &r.evidence.source_id)
                }));
                if count < usize::from(PAGE) {
                    break;
                }
                offset += u64::from(PAGE);
            }
        }
        self.references.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(())
    }
}

impl<R: GraphReader> Search<'_, R> {
    fn evidence_refs(&self) -> QueryResult<Vec<EvidenceRef>> {
        let mut refs = BTreeMap::new();
        for r in self
            .relations
            .values()
            .filter_map(|r| r.reference.as_ref())
            .chain(self.references.iter())
        {
            refs.insert(
                (
                    r.evidence.source_id.clone(),
                    r.evidence.span.start,
                    r.evidence.span.end,
                    r.evidence.reference_id.clone(),
                ),
                r.evidence.clone(),
            );
        }
        let definitions = self
            .nodes
            .values()
            .cloned()
            .chain(
                self.locations
                    .iter()
                    .flat_map(|l| l.candidates.iter().cloned()),
            )
            .collect::<Vec<_>>();
        for n in definitions {
            let source = self.reader.source(&n.source_id, self.ctx)?;
            refs.insert(
                (n.source_id.clone(), n.span.start, n.span.end, None),
                EvidenceRef {
                    source_id: n.source_id,
                    raw_digest: source
                        .raw_digest
                        .ok_or_else(|| QueryError::invalid("node source digest missing"))?,
                    decoded_digest: source
                        .decoded_digest
                        .ok_or_else(|| QueryError::invalid("node decoded digest missing"))?,
                    span: n.span,
                    reference_id: None,
                },
            );
        }
        for location in &self.locations {
            if let Locator::Position {
                source_id,
                byte_offset,
                ..
            } = &location.locator
                && !location.candidates.is_empty()
            {
                let source = self.reader.source(source_id, self.ctx)?;
                refs.insert(
                    (source_id.clone(), *byte_offset, *byte_offset, None),
                    EvidenceRef {
                        source_id: source_id.clone(),
                        raw_digest: source
                            .raw_digest
                            .ok_or_else(|| QueryError::invalid("position source digest missing"))?,
                        decoded_digest: source.decoded_digest.ok_or_else(|| {
                            QueryError::invalid("position decoded digest missing")
                        })?,
                        span: super::ByteSpan {
                            start: *byte_offset,
                            end: *byte_offset,
                        },
                        reference_id: None,
                    },
                );
            }
        }
        Ok(refs.into_values().collect())
    }
}

fn read_needs(request: &ExploreRequest) -> QueryResult<GraphReadNeeds> {
    if request.domain != QueryDomain::Code {
        return Err(QueryError::new(
            QueryErrorKind::Unsupported,
            "explore",
            "only code exploration is available",
        ));
    }
    let relations = match &request.operation {
        ExploreOperation::RelationTypes { .. }
        | ExploreOperation::Semantic { .. }
        | ExploreOperation::Locate { .. } => vec![],
        ExploreOperation::Neighbors { relations, .. } => relations.clone(),
        ExploreOperation::Expand { traversal, .. }
        | ExploreOperation::ShortestPath { traversal, .. } => traversal.relations.clone(),
        ExploreOperation::References { .. } => vec![
            RelationKind::Calls,
            RelationKind::Imports,
            RelationKind::Extends,
            RelationKind::Implements,
        ],
    };
    if relations.iter().any(|k| {
        !matches!(
            k,
            RelationKind::Contains
                | RelationKind::Calls
                | RelationKind::Imports
                | RelationKind::Extends
                | RelationKind::Implements
        )
    }) {
        return Err(QueryError::new(
            QueryErrorKind::Unsupported,
            "explore",
            "code relation unsupported",
        ));
    }
    Ok(GraphReadNeeds {
        domain: request.domain,
        relations,
        text_mapping: false,
    })
}
