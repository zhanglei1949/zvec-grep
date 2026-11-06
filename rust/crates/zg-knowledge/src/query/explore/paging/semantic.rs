use super::{
    BudgetUsage, EpisodeState, ExploreRequest, GraphReader, Identity, KgReadView, ObjectId,
    QueryError, QueryErrorKind, QueryResult, RelationKind, RelationRecord, RequestContext,
    SemanticObject, SemanticSourceState, SemanticValue, filter_data, object_ref, permits_kind,
    permits_source, query_for, request_anchor,
};
pub(super) fn edge<L, R: GraphReader>(
    reader: &R,
    view: &KgReadView<L>,
    request: &ExploreRequest,
    edge: &mut RelationRecord,
    ctx: &RequestContext,
) -> QueryResult<Option<Vec<SemanticObject>>> {
    let access = view.semantic_access()?;
    let endpoint_space = match &edge.target_object {
        Some(ObjectId::SemanticEntity { identity } | ObjectId::Episode { identity }) => {
            &identity.space_id
        }
        _ => return Ok(None),
    };
    if access
        .spaces
        .as_ref()
        .is_some_and(|spaces| !spaces.contains(endpoint_space))
    {
        return Ok(None);
    }
    match edge.kind {
        RelationKind::SemanticRelation => fact_edge(reader, view, request, edge, ctx),
        RelationKind::Mentions => mention_edge(reader, view, request, edge, ctx),
        RelationKind::EpisodeLink => episode_edge(reader, view, request, edge, ctx),
        _ => Err(QueryError::invalid("semantic adjacency required")),
    }
}
fn fact_edge<L, R: GraphReader>(
    reader: &R,
    view: &KgReadView<L>,
    request: &ExploreRequest,
    edge: &mut RelationRecord,
    ctx: &RequestContext,
) -> QueryResult<Option<Vec<SemanticObject>>> {
    let access = view.semantic_access()?;
    let (space_id, id) =
        serde_json::from_str(&edge.id).map_err(|_| QueryError::invalid("invalid fact identity"))?;
    let query = query_for(
        view.stamp(),
        ObjectId::Fact {
            identity: Identity { space_id, id },
        },
    )?;
    let Some(mut data) = reader.semantic_selection(&query, ctx)? else {
        return Err(QueryError::new(
            QueryErrorKind::Unsupported,
            "neighbors",
            "targeted fact reads unavailable",
        ));
    };
    filter_data(&mut data, access, request);
    let Some(fact) = data
        .current_facts()
        .find(|f| {
            f.identity
                == match &query.anchor.as_ref().expect("anchor").object {
                    ObjectId::Fact { identity } => identity.clone(),
                    _ => unreachable!(),
                }
        })
        .cloned()
    else {
        return Ok(None);
    };
    let objects = crate::query::semantic_read::select(
        &data,
        &query,
        access,
        &view.stamp().require_graph()?.build_id,
        view.stamp(),
        ctx,
    )?
    .objects;
    let destination = if edge.source_object.as_ref() == request_anchor(request) {
        edge.target_object.as_ref()
    } else {
        edge.source_object.as_ref()
    };
    let kind = match destination {
        Some(ObjectId::SemanticEntity { identity }) => data
            .entities
            .iter()
            .find(|e| &e.identity == identity)
            .map(|e| e.kind.as_str()),
        _ => None,
    };
    if kind.is_none_or(|kind| {
        !permits_kind(&request.boundary, kind) || !permits_kind(&request.result_filter, kind)
    }) {
        return Ok(None);
    }
    edge.fact = Some(fact);
    Ok(Some(objects))
}
fn mention_edge<L, R: GraphReader>(
    reader: &R,
    view: &KgReadView<L>,
    request: &ExploreRequest,
    edge: &mut RelationRecord,
    ctx: &RequestContext,
) -> QueryResult<Option<Vec<SemanticObject>>> {
    let access = view.semantic_access()?;
    let mut objects = Vec::new();
    let Some(mention) = &edge.mention else {
        return Err(QueryError::invalid("mention evidence missing"));
    };
    if !permits_source(&access.sources, &mention.evidence.source_id)
        || !permits_source(&request.boundary, &mention.evidence.source_id)
        || !permits_source(&request.result_filter, &mention.evidence.source_id)
    {
        return Ok(None);
    }
    let object = ObjectId::SemanticEntity {
        identity: mention.target.clone(),
    };
    let query = query_for(view.stamp(), object.clone())?;
    let Some(mut data) = reader.semantic_object(&query, ctx)? else {
        return Err(QueryError::new(
            QueryErrorKind::Unsupported,
            "neighbors",
            "targeted entity reads unavailable",
        ));
    };
    filter_data(&mut data, access, request);
    if !data.sources.iter().any(|s| {
        s.source.source_id == mention.evidence.source_id
            && s.source.source_version == mention.evidence.raw_digest
            && s.state != SemanticSourceState::Deleted
            && s.semantic_version.as_ref() == Some(&s.source.source_version)
    }) {
        return Ok(None);
    }
    let selected = crate::query::semantic_read::select(
        &data,
        &query,
        access,
        &view.stamp().require_graph()?.build_id,
        view.stamp(),
        ctx,
    )?;
    let Some(entity_kind) = selected.objects.iter().find_map(|o| match &o.record {
        SemanticValue::Entity(entity) if o.reference.object == object => Some(entity.kind.clone()),
        _ => None,
    }) else {
        return Ok(None);
    };
    let nodes = reader.source_nodes(std::slice::from_ref(&mention.owner_id), ctx)?;
    ctx.charge(BudgetUsage {
        nodes: nodes.len() as u64,
        ..Default::default()
    })?;
    if nodes.len() != 1 {
        return Err(QueryError::new(
            QueryErrorKind::ViewMismatch,
            "neighbors",
            "mention owner missing",
        ));
    }
    let destination_kind = if edge.source_object.as_ref() == request_anchor(request) {
        entity_kind
    } else {
        serde_json::to_value(nodes[0].kind)
            .ok()
            .and_then(|value| value.as_str().map(str::to_owned))
            .ok_or_else(|| QueryError::invalid("unknown source node kind"))?
    };
    if !permits_kind(&request.boundary, &destination_kind)
        || !permits_kind(&request.result_filter, &destination_kind)
    {
        return Ok(None);
    }
    objects.extend(
        selected
            .objects
            .into_iter()
            .filter(|o| o.reference.object == object),
    );
    objects.push(SemanticObject {
        reference: object_ref(
            view.stamp(),
            ObjectId::SourceNode {
                node_id: mention.owner_id.clone(),
            },
        )?,
        record: SemanticValue::SourceNode(nodes.into_iter().next().expect("checked")),
        provenance: vec![],
    });
    Ok(Some(objects))
}
fn episode_edge<L, R: GraphReader>(
    reader: &R,
    view: &KgReadView<L>,
    request: &ExploreRequest,
    edge: &mut RelationRecord,
    ctx: &RequestContext,
) -> QueryResult<Option<Vec<SemanticObject>>> {
    let access = view.semantic_access()?;
    let mut objects = Vec::new();
    for object in [&edge.source_object, &edge.target_object]
        .into_iter()
        .flatten()
    {
        let ObjectId::Episode { identity } = object else {
            continue;
        };
        let query = query_for(view.stamp(), object.clone())?;
        let Some(mut data) = reader.semantic_object(&query, ctx)? else {
            return Err(QueryError::new(
                QueryErrorKind::Unsupported,
                "neighbors",
                "targeted episode reads unavailable",
            ));
        };
        filter_data(&mut data, access, request);
        let Some(episode) = data.episodes.iter().find(|e| {
            &e.identity == identity
                && e.state == EpisodeState::Active
                && data
                    .sources
                    .iter()
                    .any(|s| s.source == e.source && s.state != SemanticSourceState::Deleted)
        }) else {
            return Ok(None);
        };
        objects.push(SemanticObject {
            reference: object_ref(view.stamp(), object.clone())?,
            record: SemanticValue::Episode(episode.clone()),
            provenance: vec![crate::query::SemanticProvenance {
                evidence: crate::semantic::SourceEvidence {
                    source: episode.source.clone(),
                    episode: Some(identity.clone()),
                    locator: None,
                },
                role: crate::semantic::SourceRole::Support,
                read: None,
                unavailable_reason: Some("body_reference_requires_host_versioned_content".into()),
            }],
        });
    }
    let (space_id, id) = serde_json::from_str(&edge.id)
        .map_err(|_| QueryError::invalid("invalid episode link identity"))?;
    let (
        Some(ObjectId::Episode { identity: source }),
        Some(ObjectId::Episode { identity: target }),
    ) = (&edge.source_object, &edge.target_object)
    else {
        return Err(QueryError::invalid("invalid episode endpoints"));
    };
    let relation_type = match edge.relation_type.as_deref() {
        Some("REPLY_TO") => crate::semantic::EpisodeRelation::ReplyTo,
        Some("RESULT_OF") => crate::semantic::EpisodeRelation::ResultOf,
        _ => return Err(QueryError::invalid("invalid episode relation type")),
    };
    edge.episode_link = Some(crate::semantic::EpisodeLink {
        identity: Identity { space_id, id },
        source: source.clone(),
        target: target.clone(),
        relation_type,
    });
    let destination = if edge.source_object.as_ref() == request_anchor(request) {
        edge.target_object.as_ref()
    } else {
        edge.source_object.as_ref()
    };
    if !objects.iter().any(|o|Some(&o.reference.object)==destination && matches!(&o.record,SemanticValue::Episode(e) if permits_kind(&request.boundary,&e.kind) && permits_kind(&request.result_filter,&e.kind))) {return Ok(None);}
    Ok(Some(objects))
}
