//! Deterministic business traversal. All reads share one view and budget.
mod augment;
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
    resolve_scope,
};
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    time::Instant,
};

fn permits(filters: &[&SourceFilter], source: &str) -> bool {
    filters.iter().all(|f| {
        f.sources
            .as_ref()
            .is_none_or(|s| s.iter().any(|s| s == source))
    })
}
fn node(view: &ViewStamp, r: &EntityRecord, distance: Option<u32>) -> QueryResult<GraphNode> {
    Ok(GraphNode {
        node: NodeRef {
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
            if count < 256 {
                break;
            }
            offset += 256;
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
        for batch in ids.chunks(256) {
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
        for batch in frontier.chunks(256) {
            for kind in kinds {
                let mut offset = 0;
                loop {
                    let page = self
                        .reader
                        .neighbors(batch, *kind, direction, offset, self.ctx)?;
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
                    let allowed = self
                        .get_nodes(&ids)?
                        .into_iter()
                        .filter(|n| permits(boundary, &n.source_id))
                        .map(|n| n.id)
                        .collect::<BTreeSet<_>>();
                    for edge in page {
                        if allowed.contains(&edge.source) && allowed.contains(&edge.target) {
                            edges.insert(edge.id.clone(), edge);
                        }
                    }
                    if count < 256 {
                        break;
                    }
                    offset += 256;
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
        if spec.include_start && permits(filter, &start.source_id) {
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
                if depth >= spec.min_hops && permits(filter, &record.source_id) {
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
        if !permits(filter, &target.source_id) {
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

fn account_output(
    result: &mut ExploreResult,
    context: &RequestContext,
    list: bool,
) -> QueryResult<()> {
    loop {
        context.check()?;
        // Reserve the maximum width for qualification timing populated by the
        // host after this pure operation. Other counters reach a fixed point.
        let qualification = result.execution.qualification_micros;
        result.execution.qualification_micros = u64::MAX;
        let mut bytes = 0;
        loop {
            result.execution.usage.output_bytes = context.usage()?.output_bytes + bytes;
            let actual = serde_json::to_vec(result)
                .map_err(|_| QueryError::invalid("result serialization failed"))?
                .len() as u64;
            if actual == bytes {
                break;
            }
            bytes = actual;
        }
        result.execution.qualification_micros = qualification;
        if bytes
            <= context
                .limits()
                .output_bytes
                .saturating_sub(context.usage()?.output_bytes)
        {
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
                if let Some(record) = records.first().filter(|n| permits(filter, &n.source_id)) {
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
                if count < 256 {
                    break;
                }
                offset += 256;
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
        ExploreOperation::Locate { .. } => vec![],
        ExploreOperation::Neighbors { relations, .. } => relations.clone(),
        ExploreOperation::Expand { traversal, .. }
        | ExploreOperation::ShortestPath { traversal, .. } => traversal.relations.clone(),
        ExploreOperation::References { .. } => vec![RelationKind::Calls, RelationKind::Imports],
    };
    if relations.iter().any(|k| {
        !matches!(
            k,
            RelationKind::Contains | RelationKind::Calls | RelationKind::Imports
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
