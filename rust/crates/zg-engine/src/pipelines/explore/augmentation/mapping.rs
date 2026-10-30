//! Actual fragment identity and content-to-source mapping.
use super::{EngineError, EngineResult, GraphSkippedHit, RetainedHit, kg, query_error};
use crate::pipelines::explore::QualifiedView;
pub(super) fn map_hits(
    selected: &QualifiedView,
    hits: &[RetainedHit],
    context: &kg::RequestContext,
) -> EngineResult<(kg::RankedHits, Vec<GraphSkippedHit>)> {
    use kg::GraphReader;
    let mut mapped = Vec::new();
    let mut skipped = Vec::new();
    for hit in hits {
        context.check().map_err(query_error)?;
        let path = hit.file.relative_path.to_path_buf();
        let Some(source_id) = path.to_str().filter(|p| {
            !p.contains(['\\', ':', '\0']) && p.split('/').all(|c| !matches!(c, "" | "." | ".."))
        }) else {
            skipped.push(skip(hit, "unsupported_source_path"));
            continue;
        };
        let source = selected
            .provider
            .lease
            .reader
            .source(source_id, context)
            .map_err(query_error)?;
        let Some(bytes) = hit.content_bytes else {
            skipped.push(skip(hit, "non_text_hit"));
            continue;
        };
        if source.language.is_none() {
            skipped.push(skip(hit, "unsupported_domain"));
            continue;
        }
        if source.record.file_id != hit.file.id.get()
            || source.record.content_hash != hit.file.snapshot.content_hash
        {
            return Err(EngineError::storage_failure(
                "retrieved source binding changed",
            ));
        }
        hit.verify(selected.provider.lease.read.storage())?;
        let range = match hit.fragment.range {
            crate::domain::Range::Full => kg::ByteSpan {
                start: 0,
                end: bytes,
            },
            crate::domain::Range::Byte(range) => kg::ByteSpan {
                start: range.start_offset(),
                end: range.end_offset(),
            },
            crate::domain::Range::Text(_) => {
                return Err(EngineError::storage_failure(
                    "fragment uses source coordinates",
                ));
            }
        };
        let Some(spans) = source_spans(&source, hit.entity_id.as_str(), range, context)? else {
            skipped.push(skip(hit, "mapping_incomplete"));
            continue;
        };
        mapped.push(kg::RankedHit {
            source_id: source_id.into(),
            entity_id: hit.entity_id.as_str().into(),
            fragment_id: hit.fragment.id.as_str().into(),
            source_spans: spans,
            priority: hit
                .priority
                .try_into()
                .map_err(|_| EngineError::internal("priority overflow"))?,
            query_sources: hit.query_sources.clone(),
        });
    }
    Ok((
        kg::RankedHits {
            view: selected.view.stamp().clone(),
            hits: mapped,
        },
        skipped,
    ))
}

fn skip(hit: &RetainedHit, reason: &str) -> GraphSkippedHit {
    GraphSkippedHit {
        entity_id: hit.entity_id.as_str().into(),
        fragment_id: hit.fragment.id.as_str().into(),
        reason: reason.into(),
    }
}

fn source_spans(
    source: &kg::SourceRecord,
    entity: &str,
    range: kg::ByteSpan,
    context: &kg::RequestContext,
) -> EngineResult<Option<Vec<kg::ByteSpan>>> {
    let mut maps = source
        .text_map
        .iter()
        .filter(|m| m.entity_id == entity)
        .collect::<Vec<_>>();
    maps.sort_by_key(|m| m.content_span.start);
    let mut cursor = range.start;
    let mut spans = Vec::new();
    for map in maps {
        context
            .charge(kg::BudgetUsage {
                mapping_units: 1,
                ..Default::default()
            })
            .map_err(query_error)?;
        map.validate().map_err(query_error)?;
        if map.content_span.end <= cursor {
            continue;
        }
        if map.content_span.start > cursor {
            return Ok(None);
        }
        let end = range.end.min(map.content_span.end);
        let start = map.source_span.start + cursor - map.content_span.start;
        spans.push(kg::ByteSpan {
            start,
            end: start + end - cursor,
        });
        cursor = end;
        if cursor == range.end {
            return Ok(Some(normalize(spans)));
        }
    }
    Ok(None)
}

fn normalize(mut spans: Vec<kg::ByteSpan>) -> Vec<kg::ByteSpan> {
    spans.sort_by_key(|span| (span.start, span.end));
    let mut merged: Vec<kg::ByteSpan> = Vec::new();
    for span in spans {
        if let Some(last) = merged.last_mut().filter(|last| span.start <= last.end) {
            last.end = last.end.max(span.end);
        } else {
            merged.push(span);
        }
    }
    merged
}

#[cfg(test)]
mod tests {
    use super::*;
    fn source() -> kg::SourceRecord {
        kg::SourceRecord {
            record: zg_knowledge::relations::bound::FileRecord {
                path: "a.rs".into(),
                file_id: 1,
                index_status: zg_knowledge::relations::bound::IndexStatus::Indexed,
                content_hash: None,
                size_bytes: 13,
                text_revision: 1,
                storage_generation: "generation".into(),
                entity_count: 1,
                entity_ids_digest: "digest".into(),
            },
            language: Some(zg_knowledge::relations::Language::Rust),
            complete: true,
            raw_digest: None,
            decoded_digest: None,
            text_binding_digest: None,
            text_map: [(2, 5, 0, 3), (7, 11, 3, 7), (11, 13, 7, 9)]
                .into_iter()
                .map(|(a, b, c, d)| kg::TextMap {
                    source_id: "a.rs".into(),
                    entity_id: "entity".into(),
                    source_span: kg::ByteSpan { start: a, end: b },
                    content_span: kg::ByteSpan { start: c, end: d },
                })
                .collect(),
        }
    }
    fn context(units: u64) -> kg::RequestContext {
        kg::RequestContext::new(
            kg::BudgetLimits {
                mapping_units: units,
                ..Default::default()
            },
            tokio_util::sync::CancellationToken::new(),
            std::time::Instant::now() + std::time::Duration::from_secs(30),
        )
        .expect("context")
    }
    #[test]
    fn fragment_maps_keep_unicode_crlf_and_noncontiguous_source_intervals() {
        let source = source();
        for map in &source.text_map {
            map.validate_text("xx中--😀\r\n", "中😀\r\n")
                .expect("real bytes");
        }
        let spans = source_spans(
            &source,
            "entity",
            kg::ByteSpan { start: 0, end: 9 },
            &context(10),
        )
        .expect("mapping")
        .expect("complete");
        assert_eq!(
            spans,
            [
                kg::ByteSpan { start: 2, end: 5 },
                kg::ByteSpan { start: 7, end: 13 }
            ]
        );
        let spans = source_spans(
            &source,
            "entity",
            kg::ByteSpan { start: 3, end: 9 },
            &context(10),
        )
        .expect("mapping")
        .expect("complete");
        assert_eq!(spans, [kg::ByteSpan { start: 7, end: 13 }]);
    }
    #[test]
    fn missing_map_and_budget_exhaustion_cannot_look_like_empty_body() {
        let mut source = source();
        let range = kg::ByteSpan { start: 0, end: 9 };
        let error =
            source_spans(&source, "entity", range, &context(1)).expect_err("shared map budget");
        assert_eq!(error.code(), EngineError::RESOURCE_BUSY);
        source.text_map.pop();
        assert!(
            source_spans(&source, "entity", range, &context(10))
                .expect("mapping")
                .is_none()
        );
        source.text_map.clear();
        assert!(
            source_spans(&source, "entity", range, &context(10))
                .expect("mapping")
                .is_none()
        );
    }
}
