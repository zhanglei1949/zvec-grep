use super::{ScopeRegion, ScopeTextEntity, ScopeTextProvider, ScopeUnit};
use crate::query::{
    BudgetUsage, ByteSpan, GraphReader, QueryError, QueryErrorKind, QueryResult, RequestContext,
    TextMap,
};
use std::collections::{BTreeMap, BTreeSet};

pub(super) fn units<R: GraphReader, T: ScopeTextProvider>(
    reader: &R,
    text: &T,
    regions: &[ScopeRegion],
    ctx: &RequestContext,
) -> QueryResult<Vec<ScopeUnit>> {
    let mut result = BTreeMap::new();
    let sources = regions
        .iter()
        .map(|r| r.source_id.clone())
        .collect::<BTreeSet<_>>();
    for source_id in sources {
        ctx.check()?;
        let source = reader.source(&source_id, ctx)?;
        if !source.complete || source.text_binding_digest.is_none() {
            return Err(QueryError::new(
                QueryErrorKind::ViewMismatch,
                "scope",
                "scope_mapping_unavailable",
            ));
        }
        let allowed = regions
            .iter()
            .filter(|r| r.source_id == source_id)
            .map(|r| r.span)
            .collect::<Vec<_>>();
        let mut needed = Vec::new();
        let mut ids = BTreeSet::new();
        for map in &source.text_map {
            ctx.charge(BudgetUsage {
                mapping_units: 1,
                ..Default::default()
            })?;
            map.validate()?;
            if map.source_id != source_id {
                return Err(QueryError::invalid("scope source mapping mismatch"));
            }
            for region in &allowed {
                if let Some(span) = intersection(map.source_span, *region) {
                    needed.push(span);
                    ids.insert(map.entity_id.clone());
                }
            }
        }
        let mut covered = Vec::new();
        for batch in ids.into_iter().collect::<Vec<_>>().chunks(256) {
            let entities = text.text_entities(batch, ctx)?;
            if entities.len() != batch.len() || entities.iter().any(|e| !batch.contains(&e.id)) {
                return Err(QueryError::invalid("scope text identity mismatch"));
            }
            for entity in entities {
                for unit in safe_units(&source_id, &entity, &source.text_map, &allowed, ctx)? {
                    covered.extend(unit.source_spans.iter().copied());
                    result.insert(unit.fragment_id.clone(), unit);
                }
            }
        }
        let covered = normalize(covered);
        if needed.iter().any(|span| !contained(&covered, *span)) {
            return Err(QueryError::new(
                QueryErrorKind::Unsupported,
                "scope",
                "scope_not_representable",
            ));
        }
    }
    Ok(result.into_values().collect())
}

fn safe_units(
    source: &str,
    entity: &ScopeTextEntity,
    maps: &[TextMap],
    allowed: &[ByteSpan],
    ctx: &RequestContext,
) -> QueryResult<Vec<ScopeUnit>> {
    let mut result = Vec::new();
    let mut maps = maps
        .iter()
        .filter(|m| m.entity_id == entity.id)
        .collect::<Vec<_>>();
    maps.sort_by_key(|m| m.content_span.start);
    let allowed = normalize(allowed.to_vec());
    for fragment in &entity.fragments {
        ctx.charge(BudgetUsage {
            mapping_units: 1,
            candidates: 1,
            ..Default::default()
        })?;
        fragment.span.validate()?;
        if fragment.span.end > entity.content_bytes {
            return Err(QueryError::invalid("scope fragment outside body"));
        }
        let spans = map_fragment(fragment.span, &maps, ctx)?;
        if !spans.is_empty() && spans.iter().all(|s| contained(&allowed, *s)) {
            result.push(ScopeUnit {
                source_id: source.into(),
                entity_id: entity.id.clone(),
                fragment_id: fragment.id.clone(),
                content_span: fragment.span,
                source_spans: spans,
            });
        }
    }
    Ok(result)
}

fn map_fragment(
    span: ByteSpan,
    maps: &[&TextMap],
    ctx: &RequestContext,
) -> QueryResult<Vec<ByteSpan>> {
    let mut cursor = span.start;
    let mut spans = Vec::new();
    for map in maps {
        ctx.charge(BudgetUsage {
            mapping_units: 1,
            ..Default::default()
        })?;
        if map.content_span.end <= cursor {
            continue;
        }
        if cursor >= span.end {
            break;
        }
        if map.content_span.start > cursor {
            return Err(QueryError::new(
                QueryErrorKind::ViewMismatch,
                "scope",
                "scope_mapping_incomplete",
            ));
        }
        let end = span.end.min(map.content_span.end);
        let start = map.source_span.start + cursor - map.content_span.start;
        spans.push(ByteSpan {
            start,
            end: start + end - cursor,
        });
        cursor = end;
    }
    if cursor != span.end {
        return Err(QueryError::new(
            QueryErrorKind::ViewMismatch,
            "scope",
            "scope_mapping_incomplete",
        ));
    }
    Ok(normalize(spans))
}
fn intersection(a: ByteSpan, b: ByteSpan) -> Option<ByteSpan> {
    let start = a.start.max(b.start);
    let end = a.end.min(b.end);
    (start < end).then_some(ByteSpan { start, end })
}
fn normalize(mut spans: Vec<ByteSpan>) -> Vec<ByteSpan> {
    spans.sort_by_key(|s| (s.start, s.end));
    let mut out: Vec<ByteSpan> = Vec::new();
    for span in spans {
        if let Some(last) = out.last_mut().filter(|last| span.start <= last.end) {
            last.end = last.end.max(span.end);
        } else {
            out.push(span);
        }
    }
    out
}
fn contained(spans: &[ByteSpan], span: ByteSpan) -> bool {
    spans
        .iter()
        .any(|s| s.start <= span.start && s.end >= span.end)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::query::ScopeTextFragment;
    #[test]
    fn noncontiguous_maps_preserve_gaps_and_crossing_units_are_excluded() {
        let ctx = RequestContext::new(
            crate::query::BudgetLimits::default(),
            tokio_util::sync::CancellationToken::new(),
            std::time::Instant::now() + std::time::Duration::from_secs(5),
        )
        .expect("budget");
        let maps = vec![
            TextMap {
                source_id: "a.rs".into(),
                entity_id: "e".into(),
                source_span: ByteSpan { start: 0, end: 3 },
                content_span: ByteSpan { start: 0, end: 3 },
            },
            TextMap {
                source_id: "a.rs".into(),
                entity_id: "e".into(),
                source_span: ByteSpan { start: 7, end: 10 },
                content_span: ByteSpan { start: 3, end: 6 },
            },
        ];
        let entity = ScopeTextEntity {
            id: "e".into(),
            content_bytes: 6,
            fragments: vec![ScopeTextFragment {
                id: "whole".into(),
                span: ByteSpan { start: 0, end: 6 },
            }],
        };
        let units = safe_units(
            "a.rs",
            &entity,
            &maps,
            &[ByteSpan { start: 0, end: 10 }],
            &ctx,
        )
        .expect("mapped");
        assert_eq!(
            units[0].source_spans,
            vec![
                ByteSpan { start: 0, end: 3 },
                ByteSpan { start: 7, end: 10 }
            ]
        );
        assert!(
            safe_units(
                "a.rs",
                &entity,
                &maps,
                &[ByteSpan { start: 0, end: 3 }],
                &ctx
            )
            .expect("crossing")
            .is_empty()
        );
        let missing = map_fragment(ByteSpan { start: 0, end: 6 }, &[&maps[0]], &ctx)
            .expect_err("missing mapping");
        assert_eq!(missing.reason, "scope_mapping_incomplete");
    }
}
