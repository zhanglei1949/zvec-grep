//! Map actual retrieved intervals to one business owner. Names never resolve hits.
use std::collections::BTreeMap;

use super::{
    BudgetUsage, EntityKind, EntityRecord, GraphReader, KgReadView, NodeRef, QueryResult,
    RankedHit, RankedHits, RequestContext, SourceFilter, SourceRecord,
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SeedMapping {
    pub node: NodeRef,
    pub hit_indices: Vec<usize>,
    pub query_sources: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SkippedHit {
    pub hit_index: usize,
    pub reason: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SeedSelection {
    pub seeds: Vec<SeedMapping>,
    pub skipped: Vec<SkippedHit>,
}

pub fn select_seeds<L, R: GraphReader>(
    reader: &R,
    view: &KgReadView<L>,
    hits: &RankedHits,
    limit: u32,
    boundary: &SourceFilter,
    context: &RequestContext,
) -> QueryResult<SeedSelection> {
    hits.validate_for(view.stamp())?;
    boundary.validate()?;
    if !(1..=5).contains(&limit) {
        return Err(super::QueryError::invalid(
            "seed count must be between one and five",
        ));
    }
    view.require(&super::GraphReadNeeds {
        domain: super::QueryDomain::Code,
        relations: vec![],
        text_mapping: true,
    })?;
    let mut selection = SeedSelection {
        seeds: vec![],
        skipped: vec![],
    };
    let mut sources = BTreeMap::<String, SourceRecord>::new();
    let mut order = (0..hits.hits.len()).collect::<Vec<_>>();
    order.sort_by_key(|i| (hits.hits[*i].priority, *i));
    for index in order {
        let hit = &hits.hits[index];
        context.check()?;
        if boundary
            .sources
            .as_ref()
            .is_some_and(|s| !s.contains(&hit.source_id))
        {
            selection.skip(index, "boundary_excluded");
            continue;
        }
        if !sources.contains_key(&hit.source_id) {
            let source = reader.source(&hit.source_id, context)?;
            context.charge(BudgetUsage {
                mapping_units: source.text_map.len() as u64,
                ..Default::default()
            })?;
            sources.insert(hit.source_id.clone(), source);
        }
        let source = &sources[&hit.source_id];
        if source.record.path != hit.source_id || !source.complete {
            return Err(super::QueryError::new(
                super::QueryErrorKind::GraphUnavailable,
                "seed_mapping",
                "source is incomplete or does not match the hit",
            ));
        }
        if source.language.is_none() {
            selection.skip(index, "unsupported_domain");
            continue;
        }
        if !mapped(hit, source, context)? {
            selection.skip(index, "mapping_incomplete");
            continue;
        }
        match owner(reader, hit, context)? {
            Ok(owner) => selection.add(owner, index, hit, limit, view)?,
            Err(reason) => selection.skip(index, reason),
        }
    }
    Ok(selection)
}

impl SeedSelection {
    fn skip(&mut self, hit_index: usize, reason: &str) {
        self.skipped.push(SkippedHit {
            hit_index,
            reason: reason.into(),
        });
    }
    fn add<L>(
        &mut self,
        owner: EntityRecord,
        index: usize,
        hit: &RankedHit,
        limit: u32,
        view: &KgReadView<L>,
    ) -> QueryResult<()> {
        let position =
            if let Some(index) = self.seeds.iter().position(|s| s.node.node_id == owner.id) {
                index
            } else {
                if self.seeds.len() >= limit as usize {
                    self.skip(index, "seed_limit");
                    return Ok(());
                }
                self.seeds.push(SeedMapping {
                    node: NodeRef {
                        graph_build_id: view.stamp().require_graph()?.build_id.clone(),
                        node_id: owner.id,
                    },
                    hit_indices: vec![],
                    query_sources: vec![],
                });
                self.seeds.len() - 1
            };
        let seed = &mut self.seeds[position];
        seed.hit_indices.push(index);
        for query in &hit.query_sources {
            if !seed.query_sources.contains(query) {
                seed.query_sources.push(query.clone());
            }
        }
        Ok(())
    }
}

fn mapped(hit: &RankedHit, source: &SourceRecord, context: &RequestContext) -> QueryResult<bool> {
    let mut maps = source
        .text_map
        .iter()
        .filter(|m| m.entity_id == hit.entity_id && m.source_id == hit.source_id)
        .collect::<Vec<_>>();
    maps.sort_by_key(|m| m.source_span.start);
    for span in &hit.source_spans {
        let mut cursor = span.start;
        for map in &maps {
            context.charge(BudgetUsage {
                mapping_units: 1,
                ..Default::default()
            })?;
            if map.validate().is_err() {
                return Ok(false);
            }
            if map.source_span.start <= cursor && map.source_span.end > cursor {
                cursor = map.source_span.end.min(span.end);
            }
            if cursor == span.end {
                break;
            }
        }
        if cursor != span.end {
            return Ok(false);
        }
    }
    Ok(true)
}

fn owner<R: GraphReader>(
    reader: &R,
    hit: &RankedHit,
    context: &RequestContext,
) -> QueryResult<Result<EntityRecord, &'static str>> {
    let mut nodes = BTreeMap::new();
    for span in &hit.source_spans {
        let mut offset = 0;
        loop {
            let page = reader.span_nodes(&hit.source_id, *span, offset, context)?;
            context.charge(BudgetUsage {
                nodes: page.len() as u64,
                candidates: page.len() as u64,
                ..Default::default()
            })?;
            let count = page.len();
            nodes.extend(page.into_iter().map(|n| (n.id.clone(), n)));
            if count < 256 {
                break;
            }
            offset += 256;
        }
    }
    let mut containing = nodes
        .values()
        .filter(|n| {
            n.source_id == hit.source_id
                && hit
                    .source_spans
                    .iter()
                    .all(|s| n.span.start <= s.start && s.end <= n.span.end)
        })
        .collect::<Vec<_>>();
    containing.sort_by_key(|n| (n.span.end - n.span.start, &n.id));
    let Some(first) = containing.first() else {
        return Ok(Err("no_single_owner"));
    };
    if containing
        .get(1)
        .is_some_and(|next| next.span.end - next.span.start == first.span.end - first.span.start)
    {
        return Ok(Err("ambiguous_owner"));
    }
    let exact = hit.source_spans.len() == 1 && hit.source_spans[0] == first.span;
    if (!exact || matches!(first.kind, EntityKind::File | EntityKind::Module))
        && nodes.values().any(|n| {
            n.id != first.id
                && n.span != first.span
                && first.span.start <= n.span.start
                && n.span.end <= first.span.end
        })
    {
        return Ok(Err("container_hit"));
    }
    Ok(Ok((*first).clone()))
}
