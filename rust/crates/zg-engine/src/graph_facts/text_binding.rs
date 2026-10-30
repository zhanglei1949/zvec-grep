//! Exact host text/fragment identity. No vector payload or second body store.
#[cfg(feature = "knowledge-graph-graphdb")]
use crate::storage::IndexStore;
use crate::{
    EngineError, EngineResult,
    domain::{Content, Entity, Range},
};
use zg_knowledge::query::{ByteSpan, TextMap};
fn initial() -> EngineResult<String> {
    zg_knowledge::processing::stable_digest(&"host-text-bindings-v2").map_err(super::map_error)
}
fn feed(digest: &mut String, entities: &[Entity]) -> EngineResult<()> {
    for entity in entities {
        let row = (
            entity.id.as_str(),
            entity.source_range,
            &entity.content,
            entity
                .fragments
                .iter()
                .map(|f| (f.id.as_str(), f.range))
                .collect::<Vec<_>>(),
        );
        *digest =
            zg_knowledge::processing::stable_digest(&(&*digest, row)).map_err(super::map_error)?;
    }
    Ok(())
}
pub(crate) fn digest(entities: &[Entity]) -> EngineResult<String> {
    let mut sorted = entities.iter().collect::<Vec<_>>();
    sorted.sort_by(|a, b| a.id.cmp(&b.id));
    let mut digest = initial()?;
    for entity in sorted {
        feed(&mut digest, std::slice::from_ref(entity))?;
    }
    Ok(digest)
}
#[cfg(feature = "knowledge-graph-graphdb")]
pub(crate) fn stored_digest(storage: &IndexStore, ids: &[String]) -> EngineResult<String> {
    let mut sorted = ids.to_vec();
    sorted.sort();
    let mut digest = initial()?;
    for batch in sorted.chunks(256) {
        feed(&mut digest, &storage.graph_entities(batch)?)?;
    }
    Ok(digest)
}
#[cfg(feature = "knowledge-graph-graphdb")]
pub(crate) fn verify(
    storage: &IndexStore,
    ids: &[String],
    source: &str,
    expected: &[TextMap],
    expected_digest: Option<&str>,
) -> EngineResult<bool> {
    let mut sorted = ids.to_vec();
    sorted.sort();
    let mut digest = initial()?;
    let mut position = 0;
    for batch in sorted.chunks(256) {
        let entities = zg_knowledge::graph::profile::measure("qualification.text_fetch", || {
            storage.graph_entities(batch)
        })?;
        zg_knowledge::graph::profile::count("qualification.text_entities", entities.len() as u64);
        zg_knowledge::graph::profile::count(
            "qualification.text_fragments",
            entities.iter().map(|e| e.fragments.len() as u64).sum(),
        );
        zg_knowledge::graph::profile::count(
            "qualification.text_bytes",
            entities
                .iter()
                .map(|e| match &e.content {
                    Content::Text(text) => text.len() as u64,
                    _ => 0,
                })
                .sum(),
        );
        let _digest = zg_knowledge::graph::profile::stage("qualification.text_digest");
        feed(&mut digest, &entities)?;
        let actual = maps(source, &entities)?;
        if expected.get(position..position + actual.len()) != Some(actual.as_slice()) {
            return Ok(false);
        }
        position += actual.len();
    }
    Ok(position == expected.len() && expected_digest == Some(digest.as_str()))
}
pub(crate) fn maps(source: &str, entities: &[Entity]) -> EngineResult<Vec<TextMap>> {
    let mut maps = entities
        .iter()
        .filter_map(|entity| {
            let Content::Text(text) = &entity.content else {
                return None;
            };
            if text.is_empty() {
                return None;
            }
            Some((|| {
                let Range::Text(range) = entity.source_range else {
                    return Err(EngineError::storage_failure(
                        "graph text lacks source mapping",
                    ));
                };
                let map = TextMap {
                    source_id: source.into(),
                    source_span: ByteSpan {
                        start: range.start_byte_offset() as u64,
                        end: range.end_byte_offset() as u64,
                    },
                    entity_id: entity.id.as_str().into(),
                    content_span: ByteSpan {
                        start: 0,
                        end: text.len() as u64,
                    },
                };
                map.validate()
                    .map_err(|_| EngineError::storage_failure("invalid extracted TextMap"))?;
                Ok(map)
            })())
        })
        .collect::<EngineResult<Vec<_>>>()?;
    maps.sort_by(|a, b| {
        a.entity_id
            .cmp(&b.entity_id)
            .then(a.content_span.start.cmp(&b.content_span.start))
    });
    Ok(maps)
}
