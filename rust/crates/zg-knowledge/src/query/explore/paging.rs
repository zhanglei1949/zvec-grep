mod semantic;
use super::{
    BTreeMap, BTreeSet, BudgetUsage, Completion, CoverageDiagnostic, Direction, EvidenceProvider,
    EvidenceRead, ExploreOperation, ExploreRequest, ExploreResult, GraphReadNeeds, GraphReader,
    Instant, KgReadView, Locator, QueryDomain, QueryError, QueryErrorKind, QueryExecution,
    QueryResult, RelationKind, RelationRecord, RequestContext, Search, SourceFilter, ViewStamp,
    account_output,
};
use crate::{
    processing::stable_digest,
    query::{
        AvailableRelationType, NeighborRead, ObjectRef, RelationSelector, SemanticAccess,
        SemanticObject, SemanticQuery, SemanticReadResult, SemanticValue, pagination::Cursor,
    },
    semantic::{EpisodeState, Identity, ObjectId, SemanticCatalog, SemanticSourceState},
};

fn hash(value: &impl serde::Serialize) -> QueryResult<String> {
    stable_digest(value).map_err(|_| QueryError::invalid("query fingerprint failed"))
}
fn permits_source(filter: &SourceFilter, id: &str) -> bool {
    filter
        .sources
        .as_ref()
        .is_none_or(|v| v.iter().any(|s| s == id))
}
fn permits_kind(filter: &SourceFilter, kind: &str) -> bool {
    filter
        .kinds
        .as_ref()
        .is_none_or(|v| v.iter().any(|s| s == kind))
}
fn filter_data(data: &mut SemanticCatalog, access: &SemanticAccess, request: &ExploreRequest) {
    data.sources.retain(|s| {
        permits_source(&access.sources, &s.source.source_id)
            && permits_source(&request.boundary, &s.source.source_id)
            && permits_source(&request.result_filter, &s.source.source_id)
    });
    let ids = data
        .sources
        .iter()
        .map(|s| s.source.source_id.clone())
        .collect::<BTreeSet<_>>();
    data.associations
        .retain(|a| ids.contains(&a.evidence.source.source_id));
    data.episodes.retain(|e| {
        access
            .contexts
            .as_ref()
            .is_none_or(|v| v.contains(&e.context))
            && ids.contains(&e.source.source_id)
    });
    data.facts.retain(|f| {
        access
            .contexts
            .as_ref()
            .is_none_or(|v| v.contains(&f.context))
    });
}
fn object_ref(stamp: &ViewStamp, object: ObjectId) -> QueryResult<ObjectRef> {
    Ok(ObjectRef {
        generation: stamp.generation.clone(),
        workspace_id: stamp.workspace_id.clone(),
        graph_build_id: stamp.require_graph()?.build_id.clone(),
        object,
    })
}
fn query_for(stamp: &ViewStamp, object: ObjectId) -> QueryResult<SemanticQuery> {
    let space_id = match &object {
        ObjectId::SemanticEntity { identity }
        | ObjectId::Episode { identity }
        | ObjectId::Fact { identity } => identity.space_id.clone(),
        ObjectId::SourceNode { .. } => {
            return Err(QueryError::invalid("semantic object identity required"));
        }
    };
    Ok(SemanticQuery {
        space_id,
        anchor: Some(object_ref(stamp, object)?),
        context: None,
        name: None,
        kind: None,
        relation_type: None,
        history: false,
        direction: Direction::Both,
    })
}

fn request_anchor(request: &ExploreRequest) -> Option<&ObjectId> {
    let (ExploreOperation::Neighbors {
        anchor: locator, ..
    }
    | ExploreOperation::RelationTypes {
        anchor: locator, ..
    }) = &request.operation
    else {
        return None;
    };
    if let Locator::Object { object } = locator {
        Some(&object.object)
    } else {
        None
    }
}
#[derive(Clone)]
struct Frame {
    edge: RelationRecord,
    nodes: Vec<super::EntityRecord>,
    objects: Vec<SemanticObject>,
    bookmark: Cursor,
    relation_type: AvailableRelationType,
}

fn selectors(request: &ExploreRequest) -> Vec<RelationSelector> {
    let defaults = match &request.operation {
        ExploreOperation::Neighbors { relations, .. } => relations.clone(),
        _ => match request.domain {
            QueryDomain::Code => vec![
                RelationKind::Contains,
                RelationKind::Calls,
                RelationKind::Imports,
                RelationKind::Extends,
                RelationKind::Implements,
            ],
            QueryDomain::Documents => vec![
                RelationKind::Contains,
                RelationKind::Links,
                RelationKind::Mentions,
            ],
            QueryDomain::Semantic => vec![
                RelationKind::Mentions,
                RelationKind::SemanticRelation,
                RelationKind::EpisodeLink,
            ],
        },
    };
    if request.relation_filters.is_empty() {
        defaults
            .iter()
            .map(|kind| RelationSelector {
                edge_type: *kind,
                relation_types: None,
            })
            .collect::<Vec<_>>()
    } else {
        request.relation_filters.clone()
    }
}
pub(super) fn explore<L, R: GraphReader, E: EvidenceProvider<L>>(
    reader: &R,
    evidence: &E,
    view: &KgReadView<L>,
    request: &ExploreRequest,
    ctx: &RequestContext,
) -> QueryResult<ExploreResult> {
    let started = Instant::now();
    ctx.check()?;
    let (ExploreOperation::Neighbors {
        anchor: locator, ..
    }
    | ExploreOperation::RelationTypes {
        anchor: locator, ..
    }) = &request.operation
    else {
        return Err(QueryError::invalid("paged operation required"));
    };
    let mut search = Search {
        reader,
        view: view.stamp(),
        ctx,
        nodes: BTreeMap::new(),
        relations: BTreeMap::new(),
        anchor_ready: false,
        locations: Vec::new(),
        references: Vec::new(),
        paths: Vec::new(),
    };
    let selectors = selectors(request);
    view.require(&GraphReadNeeds {
        domain: request.domain,
        relations: selectors.iter().map(|s| s.edge_type).collect(),
        text_mapping: false,
    })?;
    let mut fingerprint = request.clone();
    fingerprint.cursor = None;
    fingerprint.page_size = 64;
    let query = hash(&fingerprint)?;
    let authorization = if request.domain == QueryDomain::Semantic {
        hash(view.semantic_access()?)?
    } else {
        hash(&view.semantic_access().ok())?
    };
    let bookmark = match &request.cursor {
        Some(value) => Cursor::decode(value, view.stamp(), &query, &authorization)?,
        None => Cursor {
            view: view.stamp().clone(),
            query,
            authorization,
            partition: 0,
            after: None,
            skip_group: false,
        },
    };
    let anchor = match locator {
        Locator::Object { object } => {
            object.validate_for(view.stamp())?;
            if matches!(object.object, ObjectId::Fact { .. }) {
                return Err(QueryError::invalid("facts are not adjacency nodes"));
            }
            object.object.clone()
        }
        _ => match search.locate(
            locator,
            &if request.domain == QueryDomain::Semantic {
                vec![&request.boundary, &view.semantic_access()?.sources]
            } else {
                vec![&request.boundary]
            },
        )? {
            Some(node) => ObjectId::SourceNode { node_id: node.id },
            None => return empty(search, evidence, view, request, ctx, started),
        },
    };
    let page = scan_page(&mut search, view, request, &anchor, &selectors, bookmark)?;
    render_page(search, evidence, view, request, page, started)
}
struct Page {
    frames: Vec<Frame>,
    bookmark: Cursor,
    reason: Option<String>,
}

fn code_edge<R: GraphReader>(
    search: &Search<'_, R>,
    request: &ExploreRequest,
    anchor: &ObjectId,
    edge: &mut RelationRecord,
) -> QueryResult<Option<Vec<super::EntityRecord>>> {
    let nodes = search.get_nodes(
        &[edge.source.clone(), edge.target.clone()]
            .into_iter()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>(),
    )?;
    let destination = if let ObjectId::SourceNode { node_id } = &anchor {
        if &edge.source == node_id {
            &edge.target
        } else {
            &edge.source
        }
    } else {
        return Err(QueryError::invalid("code adjacency needs source anchor"));
    };
    if nodes
        .iter()
        .any(|n| !permits_source(&request.boundary, &n.source_id))
        || nodes.iter().find(|n| &n.id == destination).is_none_or(|n| {
            !permits_source(&request.result_filter, &n.source_id)
                || !permits_kind(
                    &request.boundary,
                    serde_json::to_value(n.kind)
                        .ok()
                        .and_then(|v| v.as_str().map(str::to_owned))
                        .as_deref()
                        .unwrap_or(""),
                )
                || !permits_kind(
                    &request.result_filter,
                    serde_json::to_value(n.kind)
                        .ok()
                        .and_then(|v| v.as_str().map(str::to_owned))
                        .as_deref()
                        .unwrap_or(""),
                )
        })
    {
        return Ok(None);
    }
    edge.source_object
        .get_or_insert_with(|| ObjectId::SourceNode {
            node_id: edge.source.clone(),
        });
    edge.target_object
        .get_or_insert_with(|| ObjectId::SourceNode {
            node_id: edge.target.clone(),
        });
    Ok(Some(nodes))
}
#[expect(
    clippy::too_many_lines,
    reason = "keep bookmark advancement and budget stops together to prevent skipped records"
)]
fn scan_page<L, R: GraphReader>(
    search: &mut Search<'_, R>,
    view: &KgReadView<L>,
    request: &ExploreRequest,
    anchor: &ObjectId,
    selectors: &[RelationSelector],
    mut bookmark: Cursor,
) -> QueryResult<Page> {
    let ctx = search.ctx;
    let reader = search.reader;
    let discovery = matches!(request.operation, ExploreOperation::RelationTypes { .. });
    let (ExploreOperation::Neighbors { direction, .. }
    | ExploreOperation::RelationTypes { direction, .. }) = request.operation
    else {
        unreachable!()
    };
    let partitions = selectors
        .iter()
        .flat_map(|selector| {
            if discovery && direction == Direction::Both {
                vec![
                    (selector, Direction::Incoming),
                    (selector, Direction::Outgoing),
                ]
            } else {
                vec![(selector, direction)]
            }
        })
        .collect::<Vec<_>>();
    if bookmark.partition > partitions.len() {
        return Err(QueryError::invalid("cursor partition is invalid"));
    }
    let mut frames = Vec::new();
    let mut working_bytes = 0;
    let mut partial = false;
    let mut reason = None;
    'scan: while bookmark.partition < partitions.len() {
        let (selector, direction) = partitions[bookmark.partition];
        let page = match reader.neighbors_after(
            &NeighborRead {
                anchor,
                selector,
                direction,
                after: bookmark.after.as_ref(),
                skip_group: bookmark.skip_group,
                boundary: &request.boundary,
                spaces: view
                    .semantic_access()
                    .ok()
                    .and_then(|access| access.spaces.as_deref()),
                contexts: view
                    .semantic_access()
                    .ok()
                    .and_then(|access| access.contexts.as_deref()),
            },
            ctx,
        ) {
            Ok(page) => page,
            Err(error) if error.kind == QueryErrorKind::ResourceExhausted => {
                partial = true;
                reason = Some("scan_budget".into());
                break;
            }
            Err(error) => return Err(error),
        };
        let count = page.len();
        for mut edge in page {
            ctx.check()?;
            if let Err(error) = ctx.charge(BudgetUsage {
                edges: 1,
                ..Default::default()
            }) {
                if error.kind == QueryErrorKind::ResourceExhausted {
                    partial = true;
                    reason = Some("scan_budget".into());
                    break 'scan;
                }
                return Err(error);
            }
            let key = edge.key();
            if bookmark.skip_group
                && bookmark
                    .after
                    .as_ref()
                    .is_some_and(|last| last.relation_type == key.relation_type)
            {
                bookmark.after = Some(key);
                continue;
            }
            let (objects, nodes) = if matches!(
                edge.kind,
                RelationKind::SemanticRelation | RelationKind::EpisodeLink | RelationKind::Mentions
            ) {
                match semantic::edge(reader, view, request, &mut edge, ctx) {
                    Ok(Some(objects)) => (objects, Vec::new()),
                    Ok(None) => {
                        bookmark.after = Some(key);
                        bookmark.skip_group = false;
                        continue;
                    }
                    Err(error) if error.kind == QueryErrorKind::ResourceExhausted => {
                        partial = true;
                        reason = Some("scan_budget".into());
                        break 'scan;
                    }
                    Err(error) => return Err(error),
                }
            } else {
                let Some(nodes) = code_edge(search, request, anchor, &mut edge)? else {
                    bookmark.after = Some(key);
                    bookmark.skip_group = false;
                    continue;
                };
                (Vec::new(), nodes)
            };
            if frames.len() == usize::from(request.page_size) {
                partial = true;
                reason = Some("page_limit".into());
                break 'scan;
            }
            if let Err(error) =
                ctx.reserve_working_set(&mut working_bytes, &(&edge, &nodes, &objects))
            {
                if error.kind == QueryErrorKind::ResourceExhausted {
                    partial = true;
                    reason = Some("working_set_budget".into());
                    break 'scan;
                }
                return Err(error);
            }
            bookmark.after = Some(key);
            bookmark.skip_group = discovery;
            frames.push(Frame {
                relation_type: AvailableRelationType {
                    edge_type: edge.kind,
                    relation_type: edge.relation_type.clone(),
                    direction,
                },
                edge,
                nodes,
                objects,
                bookmark: bookmark.clone(),
            });
        }
        if count < 256 {
            bookmark.partition += 1;
            bookmark.after = None;
            bookmark.skip_group = false;
        }
    }
    if frames.is_empty() && partial {
        return Err(QueryError::new(
            QueryErrorKind::ResourceExhausted,
            "neighbors",
            "no resumable progress within budget",
        ));
    }
    Ok(Page {
        frames,
        bookmark,
        reason,
    })
}
#[expect(
    clippy::too_many_lines,
    reason = "page output and cursor rollback must retain the same evidence frames"
)]
fn render_page<L, R: GraphReader, E: EvidenceProvider<L>>(
    mut search: Search<'_, R>,
    evidence: &E,
    view: &KgReadView<L>,
    request: &ExploreRequest,
    page: Page,
    started: Instant,
) -> QueryResult<ExploreResult> {
    let Page {
        mut frames,
        mut bookmark,
        mut reason,
    } = page;
    let ctx = search.ctx;
    let mut partial = reason.is_some();
    // Only returned, authorized rows may appear in a client-visible bookmark.
    // Hidden/filtered tails can be scanned again; they must not leak through it.
    if partial && let Some(last) = frames.last() {
        bookmark = last.bookmark.clone();
    }
    let discovery = matches!(request.operation, ExploreOperation::RelationTypes { .. });
    loop {
        search.nodes.clear();
        search.relations.clear();
        let mut objects = BTreeMap::new();
        for frame in &frames {
            if !discovery {
                search
                    .relations
                    .insert(frame.edge.id.clone(), frame.edge.clone());
                for node in &frame.nodes {
                    search.insert(node, Some(1))?;
                }
                for object in &frame.objects {
                    objects.insert(object.reference.object.clone(), object.clone());
                }
            }
        }
        let mut refs = search.evidence_refs()?;
        refs.extend(
            frames
                .iter()
                .filter_map(|frame| frame.edge.mention.as_ref().map(|m| m.evidence.clone())),
        );
        refs.sort_by(|a, b| {
            (&a.source_id, a.span.start, a.span.end, &a.reference_id).cmp(&(
                &b.source_id,
                b.span.start,
                b.span.end,
                &b.reference_id,
            ))
        });
        refs.dedup();
        let reads = evidence.read_evidence(view, &refs, ctx)?;
        if reads.len() != refs.len()
            || reads
                .iter()
                .any(|r| matches!(r, EvidenceRead::Unavailable { .. }))
        {
            return Err(QueryError::new(
                QueryErrorKind::ViewMismatch,
                "evidence",
                "page evidence unavailable",
            ));
        }
        let mut semantic = SemanticReadResult {
            objects: objects.into_values().collect(),
            ..Default::default()
        };
        if !semantic.objects.is_empty() {
            crate::query::semantic_read::evidence(evidence, view, &mut semantic, ctx)?;
        }
        let cursor = if partial {
            Some(bookmark.encode()?)
        } else {
            None
        };
        let mut result = ExploreResult {
            next_cursor: cursor,
            relation_types: if discovery {
                frames.iter().map(|f| f.relation_type.clone()).collect()
            } else {
                Vec::new()
            },
            semantic: (!semantic.objects.is_empty()).then_some(semantic),
            view: view.stamp().clone(),
            locations: search.locations.clone(),
            nodes: search.nodes.values().cloned().collect(),
            relations: if discovery {
                Vec::new()
            } else {
                frames.iter().map(|f| f.edge.clone()).collect()
            },
            paths: Vec::new(),
            references: Vec::new(),
            evidence: reads,
            coverage: CoverageDiagnostic {
                domain: request.domain,
                ready: true,
                note: "page completeness is separate from extraction coverage".into(),
            },
            execution: QueryExecution {
                status: if partial {
                    Completion::Partial
                } else {
                    Completion::Complete
                },
                stop_reason: reason.clone(),
                usage: ctx.usage()?,
                qualification_micros: 0,
                query_micros: u64::try_from(started.elapsed().as_micros()).unwrap_or(u64::MAX),
            },
        };
        match account_output(&mut result, ctx, false) {
            Ok(()) => return Ok(result),
            Err(error) if error.kind == QueryErrorKind::ResourceExhausted && frames.len() > 1 => {
                frames.pop();
                bookmark = frames.last().expect("one retained").bookmark.clone();
                partial = true;
                reason = Some("output_budget".into());
            }
            Err(error) => return Err(error),
        }
    }
}

fn empty<L, R: GraphReader, E: EvidenceProvider<L>>(
    search: Search<'_, R>,
    evidence: &E,
    view: &KgReadView<L>,
    request: &ExploreRequest,
    ctx: &RequestContext,
    start: Instant,
) -> QueryResult<ExploreResult> {
    let refs = search.evidence_refs()?;
    let reads = evidence.read_evidence(view, &refs, ctx)?;
    if reads.len() != refs.len()
        || reads
            .iter()
            .any(|read| matches!(read, EvidenceRead::Unavailable { .. }))
    {
        return Err(QueryError::new(
            QueryErrorKind::ViewMismatch,
            "evidence",
            "anchor evidence unavailable",
        ));
    }
    let mut result = ExploreResult {
        next_cursor: None,
        relation_types: Vec::new(),
        semantic: None,
        view: view.stamp().clone(),
        locations: search.locations,
        nodes: Vec::new(),
        relations: Vec::new(),
        paths: Vec::new(),
        references: Vec::new(),
        evidence: reads,
        coverage: CoverageDiagnostic {
            domain: request.domain,
            ready: true,
            note: "anchor not uniquely located".into(),
        },
        execution: QueryExecution {
            status: Completion::Complete,
            stop_reason: None,
            usage: ctx.usage()?,
            qualification_micros: 0,
            query_micros: u64::try_from(start.elapsed().as_micros()).unwrap_or(u64::MAX),
        },
    };
    account_output(&mut result, ctx, false)?;
    Ok(result)
}
