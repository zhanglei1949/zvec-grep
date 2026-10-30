//! One augmentation call owns all seeds and shares the request budget.
use super::{
    BTreeMap, BudgetUsage, Completion, EvidenceProvider, EvidenceRead, ExploreOperation,
    ExploreRequest, GraphNode, GraphReadNeeds, GraphReader, Instant, KgReadView, Locator, NodeRef,
    QueryDomain, QueryError, QueryErrorKind, QueryExecution, QueryResult, RelationRecord,
    RequestContext, Search, SourceFilter, ViewStamp, read_needs,
};
use crate::query::{GraphQueryMode, RankedHits, SeedSelection, select_seeds};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SeedExpansion {
    pub seed: NodeRef,
    pub nodes: Vec<String>,
    pub relations: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct AugmentResult {
    pub view: ViewStamp,
    pub hits: RankedHits,
    pub selection: SeedSelection,
    pub expansions: Vec<SeedExpansion>,
    pub nodes: Vec<GraphNode>,
    pub relations: Vec<RelationRecord>,
    pub evidence: Vec<EvidenceRead>,
    pub execution: QueryExecution,
}

pub fn augment<L, R: GraphReader, E: EvidenceProvider<L>>(
    reader: &R,
    evidence: &E,
    view: &KgReadView<L>,
    hits: &RankedHits,
    mode: &GraphQueryMode,
    context: &RequestContext,
) -> QueryResult<AugmentResult> {
    mode.validate_for(view.stamp())?;
    let GraphQueryMode::Augment { seeds, expansion } = mode else {
        return Err(QueryError::invalid("augment requires graph mode augment"));
    };
    let needs = read_needs(&ExploreRequest {
        domain: QueryDomain::Code,
        operation: ExploreOperation::Expand {
            anchor: Locator::Name {
                name: "seed".into(),
                source_id: None,
                kind: None,
            },
            traversal: expansion.clone(),
        },
        boundary: SourceFilter::default(),
        result_filter: SourceFilter::default(),
    })?;
    view.require(&GraphReadNeeds {
        text_mapping: true,
        ..needs
    })?;
    let started = Instant::now();
    let selection = select_seeds(reader, view, hits, *seeds, &expansion.boundary, context)?;
    let mut merged = search(reader, view, context);
    let mut expansions = Vec::new();
    for seed in &selection.seeds {
        let mut local = search(reader, view, context);
        let mut records = local.get_nodes(std::slice::from_ref(&seed.node.node_id))?;
        let start = records
            .pop()
            .ok_or_else(|| QueryError::invalid("seed missing"))?;
        local.expand(
            &start,
            expansion,
            &[&expansion.boundary],
            &[&expansion.result_filter],
        )?;
        expansions.push(SeedExpansion {
            seed: seed.node.clone(),
            nodes: ordered_nodes(&local.nodes),
            relations: local.relations.keys().cloned().collect(),
        });
        for (id, node) in local.nodes {
            let previous = merged.nodes.entry(id).or_insert_with(|| node.clone());
            previous.distance = previous.distance.min(node.distance);
        }
        merged.relations.extend(local.relations);
    }
    let refs = merged.evidence_refs()?;
    let evidence = evidence.read_evidence(view, &refs, context)?;
    if evidence.len() != refs.len() || evidence.iter().zip(&refs).any(|(read, expected)| {
        !matches!(read, EvidenceRead::Verified { reference, .. } if reference == expected)
    }) {
        return Err(QueryError::new(QueryErrorKind::ViewMismatch, "evidence", "required augmentation evidence unavailable"));
    }
    let mut result = AugmentResult {
        view: view.stamp().clone(),
        hits: hits.clone(),
        selection,
        expansions,
        nodes: merged.nodes.into_values().collect(),
        relations: merged.relations.into_values().collect(),
        evidence,
        execution: QueryExecution {
            status: Completion::Complete,
            stop_reason: None,
            usage: context.usage()?,
            qualification_micros: 0,
            query_micros: started.elapsed().as_micros().try_into().unwrap_or(u64::MAX),
        },
    };
    organize(&mut result);
    account_output(&mut result, context)?;
    Ok(result)
}

fn ordered_nodes(nodes: &BTreeMap<String, GraphNode>) -> Vec<String> {
    let mut ordered = nodes.values().collect::<Vec<_>>();
    ordered.sort_by_key(|n| (n.distance, &n.node.node_id));
    ordered
        .into_iter()
        .map(|n| n.node.node_id.clone())
        .collect()
}

fn organize(result: &mut AugmentResult) {
    let mut nodes = BTreeMap::new();
    let mut relations = BTreeMap::new();
    for (seed, expansion) in result.expansions.iter().enumerate() {
        for (position, id) in expansion.nodes.iter().enumerate() {
            nodes.entry(id.as_str()).or_insert((seed, position));
        }
        for id in &expansion.relations {
            relations.entry(id.as_str()).or_insert(seed);
        }
    }
    result.nodes.sort_by_key(|n| {
        (
            nodes.get(n.node.node_id.as_str()).copied(),
            n.node.node_id.clone(),
        )
    });
    result
        .relations
        .sort_by_key(|r| (relations.get(r.id.as_str()).copied(), r.id.clone()));
}

pub(super) fn search<'a, L, R: GraphReader>(
    reader: &'a R,
    view: &'a KgReadView<L>,
    context: &'a RequestContext,
) -> Search<'a, R> {
    Search {
        reader,
        view: view.stamp(),
        ctx: context,
        nodes: BTreeMap::new(),
        relations: BTreeMap::new(),
        anchor_ready: true,
        locations: vec![],
        references: vec![],
        paths: vec![],
    }
}

fn account_output(result: &mut AugmentResult, context: &RequestContext) -> QueryResult<()> {
    let mut bytes = 0;
    loop {
        result.execution.usage.output_bytes = context.usage()?.output_bytes + bytes;
        let actual = serde_json::to_vec(result)
            .map_err(|_| QueryError::invalid("augment serialization failed"))?
            .len() as u64;
        if actual == bytes {
            break;
        }
        bytes = actual;
    }
    context.charge(BudgetUsage {
        output_bytes: bytes,
        ..Default::default()
    })?;
    result.execution.usage = context.usage()?;
    Ok(())
}
