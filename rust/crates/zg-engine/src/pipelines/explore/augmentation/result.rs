//! Result organization and references to text already returned by retrieval.
use super::{
    ContextResult, EngineError, EngineResult, GraphSkippedHit, GraphSupplement, convert, kg,
    query_error,
};
use crate::api::context::graph_result::{GraphEvidence, GraphSeed, GraphStatus};
pub(super) fn supplement(
    augmented: kg::AugmentResult,
    mut skipped: Vec<GraphSkippedHit>,
    result: &ContextResult,
) -> EngineResult<GraphSupplement> {
    let hits = augmented.hits.hits;
    let seeds = augmented
        .selection
        .seeds
        .into_iter()
        .map(|s| {
            Ok(GraphSeed {
                node: convert(s.node)?,
                hits: s
                    .hit_indices
                    .iter()
                    .map(|i| convert(&hits[*i]))
                    .collect::<EngineResult<_>>()?,
                query_sources: s.query_sources,
            })
        })
        .collect::<EngineResult<_>>()?;
    for hit in augmented.selection.skipped {
        let original = &hits[hit.hit_index];
        skipped.push(GraphSkippedHit {
            entity_id: original.entity_id.clone(),
            fragment_id: original.fragment_id.clone(),
            reason: hit.reason,
        });
    }
    let evidence = augmented
        .evidence
        .into_iter()
        .map(|read| organize_evidence(read, result))
        .collect::<EngineResult<_>>()?;
    Ok(GraphSupplement {
        scope: None,
        status: GraphStatus::Complete,
        view: Some(convert(augmented.view)?),
        seeds,
        skipped,
        expansions: convert(augmented.expansions)?,
        nodes: convert(augmented.nodes)?,
        relations: convert(augmented.relations)?,
        evidence,
        diagnostics: vec![],
        execution: Some(convert(augmented.execution)?),
    })
}

fn organize_evidence(
    read: kg::EvidenceRead,
    result: &ContextResult,
) -> EngineResult<GraphEvidence> {
    let kg::EvidenceRead::Verified { reference, content } = read else {
        return Err(EngineError::resource_busy(
            "augmentation evidence unavailable",
        ));
    };
    let item = result.items.iter().find(|item| {
        item.entity_id.is_some()
            && item.relative_path.to_str() == Some(reference.source_id.as_str())
            && matches!(item.content_range,
                crate::api::context::result::ContentRange::Text { start_byte_offset, end_byte_offset, .. }
                if start_byte_offset as u64 <= reference.span.start && reference.span.end <= end_byte_offset as u64)
    });
    Ok(if let Some(item) = item {
        GraphEvidence::Retrieved {
            reference: convert(reference)?,
            entity_id: item
                .entity_id
                .clone()
                .ok_or_else(|| EngineError::internal("missing retrieved identity"))?,
            item_rank: item.rank,
        }
    } else {
        GraphEvidence::Verified {
            reference: convert(reference)?,
            content,
        }
    })
}

pub(in crate::pipelines) fn account(
    graph: &mut GraphSupplement,
    context: &kg::RequestContext,
) -> EngineResult<()> {
    loop {
        let charged = context.usage().map_err(query_error)?;
        if let Some(execution) = &mut graph.execution {
            execution.usage = convert(charged)?;
        }
        let bytes = serde_json::to_vec(graph)
            .map_err(|_| EngineError::internal("encode graph supplement"))?
            .len() as u64;
        if bytes <= charged.output_bytes {
            break;
        }
        context
            .charge(kg::BudgetUsage {
                output_bytes: bytes - charged.output_bytes,
                ..Default::default()
            })
            .map_err(query_error)?;
    }
    Ok(())
}
