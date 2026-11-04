//! Result organization retains verified evidence independently of text previews.
use super::{EngineError, EngineResult, GraphSkippedHit, GraphSupplement, kg, query_error};
use crate::api::context::graph_result::{GraphEvidence, GraphSeed, GraphStatus};
pub(super) fn supplement(
    augmented: kg::AugmentResult,
    mut skipped: Vec<GraphSkippedHit>,
) -> EngineResult<GraphSupplement> {
    let hits = augmented.hits.hits;
    let seeds = augmented
        .selection
        .seeds
        .into_iter()
        .map(|s| GraphSeed {
            node: s.node,
            hits: s.hit_indices.iter().map(|i| hits[*i].clone()).collect(),
            query_sources: s.query_sources,
        })
        .collect();
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
        .map(organize_evidence)
        .collect::<EngineResult<_>>()?;
    Ok(GraphSupplement {
        scope: None,
        status: GraphStatus::Complete,
        view: Some(augmented.view),
        seeds,
        skipped,
        expansions: augmented.expansions,
        nodes: augmented.nodes,
        relations: augmented.relations,
        evidence,
        diagnostics: vec![],
        execution: Some(augmented.execution),
    })
}

fn organize_evidence(read: kg::EvidenceRead) -> EngineResult<GraphEvidence> {
    let kg::EvidenceRead::Verified { reference, content } = read else {
        return Err(EngineError::resource_busy(
            "augmentation evidence unavailable",
        ));
    };
    // Transports may truncate ordinary items after this step. Their source
    // ranges alone cannot prove that a caller received the evidence bytes.
    Ok(GraphEvidence::Verified { reference, content })
}

pub(in crate::pipelines) fn account(
    graph: &mut GraphSupplement,
    context: &kg::RequestContext,
) -> EngineResult<()> {
    loop {
        let charged = context.usage().map_err(query_error)?;
        if let Some(execution) = &mut graph.execution {
            execution.usage = charged;
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
