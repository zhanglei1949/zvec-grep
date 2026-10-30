//! Exact scopes contain complete safe retrieval units, never client allowlists.
mod mapping;
use super::{
    Completion, Direction, GraphNode, GraphReadNeeds, GraphReader, Instant, KgReadView,
    LocateResult, QueryDomain, QueryError, QueryErrorKind, QueryExecution, QueryResult,
    RelationKind, RelationRecord, RequestContext, augment,
};
use crate::query::{BodyRegion, ByteSpan, ResolvedScope, ScopeSpec};
use serde::Serialize;

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ScopeRegion {
    pub source_id: String,
    pub span: ByteSpan,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ScopeUnit {
    pub source_id: String,
    pub entity_id: String,
    pub fragment_id: String,
    pub content_span: ByteSpan,
    pub source_spans: Vec<ByteSpan>,
}
pub struct ScopeTextFragment {
    pub id: String,
    pub span: ByteSpan,
}
pub struct ScopeTextEntity {
    pub id: String,
    pub content_bytes: u64,
    pub fragments: Vec<ScopeTextFragment>,
}
pub trait ScopeTextProvider {
    /// Return the actual same-view text bodies' lengths and fragment metadata.
    /// Batch size is at most 256; implementations verify entity identity.
    fn text_entities(
        &self,
        ids: &[String],
        context: &RequestContext,
    ) -> QueryResult<Vec<ScopeTextEntity>>;
}
#[derive(Debug, Serialize)]
pub struct ScopeResolution {
    pub scope: ResolvedScope,
    pub location: LocateResult,
    pub nodes: Vec<GraphNode>,
    pub relations: Vec<RelationRecord>,
    pub execution: QueryExecution,
}

pub fn resolve_scope<L, R: GraphReader, T: ScopeTextProvider>(
    reader: &R,
    text: &T,
    view: &KgReadView<L>,
    spec: &ScopeSpec,
    ctx: &RequestContext,
) -> QueryResult<ScopeResolution> {
    spec.validate()?;
    spec.anchor.validate_for(view.stamp())?;
    let mut relations = spec.traversal.relations.clone();
    if spec.region == BodyRegion::Own && !relations.contains(&RelationKind::Contains) {
        relations.push(RelationKind::Contains);
    }
    view.require(&GraphReadNeeds {
        domain: QueryDomain::Code,
        relations,
        text_mapping: true,
    })?;
    let started = Instant::now();
    let mut search = augment::search(reader, view, ctx);
    let anchor = search
        .locate(&spec.anchor, &[&spec.traversal.boundary])?
        .ok_or_else(|| QueryError::invalid("scope_anchor_not_unique"))?;
    search.expand(
        &anchor,
        &spec.traversal,
        &[&spec.traversal.boundary],
        &[&spec.traversal.result_filter],
    )?;
    let mut regions = Vec::new();
    for selected in search.nodes.values() {
        let children = if spec.region == BodyRegion::Own {
            let edges = search.edges(
                std::slice::from_ref(&selected.node.node_id),
                &[RelationKind::Contains],
                Direction::Outgoing,
                &[],
            )?;
            let ids = edges.into_iter().map(|e| e.target).collect::<Vec<_>>();
            search
                .get_nodes(&ids)?
                .into_iter()
                .map(|n| n.span)
                .collect()
        } else {
            vec![]
        };
        regions.extend(
            subtract(selected.span, &children)?
                .into_iter()
                .map(|span| ScopeRegion {
                    source_id: selected.source_id.clone(),
                    span,
                }),
        );
    }
    regions.sort_by_key(|r| (r.source_id.clone(), r.span.start, r.span.end));
    let units = mapping::units(reader, text, &regions, ctx)?;
    let result = ScopeResolution {
        scope: ResolvedScope {
            view: view.stamp().clone(),
            regions,
            units,
        },
        location: search
            .locations
            .pop()
            .ok_or_else(|| QueryError::invalid("missing scope location"))?,
        nodes: search.nodes.into_values().collect(),
        relations: search.relations.into_values().collect(),
        execution: QueryExecution {
            status: Completion::Complete,
            stop_reason: None,
            usage: ctx.usage()?,
            qualification_micros: 0,
            query_micros: started.elapsed().as_micros().try_into().unwrap_or(u64::MAX),
        },
    };
    ctx.check()?;
    Ok(result)
}

fn subtract(parent: ByteSpan, children: &[ByteSpan]) -> QueryResult<Vec<ByteSpan>> {
    let mut children = children.to_vec();
    children.sort_by_key(|c| (c.start, c.end));
    let mut cursor = parent.start;
    let mut regions = Vec::new();
    for child in children {
        child.validate()?;
        if child.start < parent.start || child.end > parent.end {
            return Err(QueryError::new(
                QueryErrorKind::ViewMismatch,
                "scope",
                "child outside parent",
            ));
        }
        if cursor < child.start {
            regions.push(ByteSpan {
                start: cursor,
                end: child.start,
            });
        }
        cursor = cursor.max(child.end);
    }
    if cursor < parent.end {
        regions.push(ByteSpan {
            start: cursor,
            end: parent.end,
        });
    }
    Ok(regions)
}
