//! Request-owned snapshots; every source or stored entity is read once.
use super::Lease;
use crate::domain::{Content, Entity, EntityId};
use std::{collections::HashMap, io::Read, sync::Arc};
use zg_knowledge::{
    graph::GraphDbStore,
    query::{
        BudgetUsage, ByteSpan, EvidenceProvider, EvidenceRead, EvidenceRef, GraphReader,
        KgReadView, QueryError, QueryErrorKind, QueryResult, RequestContext, SourceRecord,
    },
};
const MAX_RETAINED_BYTES: usize = 256 << 20;
pub(super) struct HostEvidence<'a> {
    pub reader: &'a GraphDbStore,
}
struct Snapshots<'a> {
    lease: &'a Lease,
    reader: &'a GraphDbStore,
    context: &'a RequestContext,
    files: HashMap<String, Option<String>>,
    entities: HashMap<String, Entity>,
    retained: usize,
}
fn unavailable(reason: &'static str) -> QueryError {
    QueryError::new(QueryErrorKind::ViewMismatch, "evidence", reason)
}
impl EvidenceProvider<Arc<Lease>> for HostEvidence<'_> {
    fn read_evidence(
        &self,
        view: &KgReadView<Arc<Lease>>,
        references: &[EvidenceRef],
        context: &RequestContext,
    ) -> QueryResult<Vec<EvidenceRead>> {
        let lease = view.lease();
        let actual = lease
            .read
            .graph_stamp(&lease.manifest)
            .map_err(|_| unavailable("view changed"))?;
        view.stamp().require_same(&actual)?;
        let mut snapshots = Snapshots {
            lease,
            reader: self.reader,
            context,
            files: HashMap::new(),
            entities: HashMap::new(),
            retained: 0,
        };
        references.iter().map(|r| snapshots.read(r)).collect()
    }
}
impl Snapshots<'_> {
    fn retain(&mut self, bytes: usize) -> QueryResult<()> {
        self.retained = self
            .retained
            .checked_add(bytes)
            .filter(|n| *n <= MAX_RETAINED_BYTES)
            .ok_or_else(|| {
                QueryError::new(
                    QueryErrorKind::ResourceExhausted,
                    "evidence",
                    "source snapshot budget exhausted",
                )
            })?;
        Ok(())
    }
    fn entity(&mut self, id: &str) -> QueryResult<&Entity> {
        if !self.entities.contains_key(id) {
            let mut found = self
                .lease
                .read
                .storage()
                .graph_entities(&[id.to_owned()])
                .map_err(|_| unavailable("mapped entity unavailable"))?;
            let entity = found
                .pop()
                .ok_or_else(|| unavailable("mapped entity unavailable"))?;
            if EntityId::new(entity.file_id, &entity.content, entity.source_range)
                .map_err(|_| unavailable("invalid stored content"))?
                != entity.id
            {
                return Err(unavailable("stored content no longer matches identity"));
            }
            if let Content::Text(content) = &entity.content {
                self.retain(content.len())?;
            }
            self.entities.insert(id.into(), entity);
        }
        self.entities
            .get(id)
            .ok_or_else(|| unavailable("entity snapshot missing"))
    }
    fn mapped(
        &mut self,
        reference: &EvidenceRef,
        source: &SourceRecord,
    ) -> QueryResult<Option<String>> {
        if reference.span.start == reference.span.end {
            for map in &source.text_map {
                if map.source_span.start <= reference.span.start
                    && reference.span.end <= map.source_span.end
                {
                    self.context.charge(BudgetUsage {
                        mapping_units: 1,
                        ..Default::default()
                    })?;
                    let point =
                        map.content_span.start + reference.span.start - map.source_span.start;
                    let entity = self.entity(&map.entity_id)?;
                    if let Content::Text(body) = &entity.content {
                        return Ok(Some(
                            ByteSpan {
                                start: point,
                                end: point,
                            }
                            .slice(body)?
                            .to_owned(),
                        ));
                    }
                }
            }
            return Ok(None);
        }
        let mut maps = source
            .text_map
            .iter()
            .filter(|m| {
                m.source_span.end > reference.span.start && m.source_span.start < reference.span.end
            })
            .collect::<Vec<_>>();
        maps.sort_by_key(|m| (m.source_span.start, m.source_span.end));
        let mut cursor = reference.span.start;
        let mut text = String::new();
        for map in maps {
            if cursor >= reference.span.end {
                break;
            }
            self.context.charge(BudgetUsage {
                mapping_units: 1,
                ..Default::default()
            })?;
            if map.source_span.start > cursor || map.source_span.end <= cursor {
                continue;
            }
            let entity = self.entity(&map.entity_id)?;
            let Content::Text(content) = &entity.content else {
                break;
            };
            let end = map.source_span.end.min(reference.span.end);
            let start = map.content_span.start + cursor - map.source_span.start;
            text.push_str(
                ByteSpan {
                    start,
                    end: start + end - cursor,
                }
                .slice(content)?,
            );
            cursor = end;
        }
        Ok((cursor == reference.span.end).then_some(text))
    }
    fn file(&mut self, reference: &EvidenceRef) -> QueryResult<Option<&str>> {
        if !self.files.contains_key(&reference.source_id) {
            let text = self.load_file(reference)?;
            if let Some(text) = &text {
                self.retain(text.len())?;
            }
            self.files.insert(reference.source_id.clone(), text);
        }
        Ok(self
            .files
            .get(&reference.source_id)
            .and_then(|t| t.as_deref()))
    }
    fn load_file(&self, reference: &EvidenceRef) -> QueryResult<Option<String>> {
        let Some(path) = self
            .lease
            .root
            .join(&reference.source_id)
            .canonicalize()
            .ok()
            .filter(|p| p.starts_with(&self.lease.root))
        else {
            return Ok(None);
        };
        let Ok(file) = std::fs::File::open(path) else {
            return Ok(None);
        };
        let remaining = MAX_RETAINED_BYTES.saturating_sub(self.retained);
        let mut bytes = Vec::new();
        if file
            .take(remaining as u64 + 1)
            .read_to_end(&mut bytes)
            .is_err()
        {
            return Ok(None);
        }
        if bytes.len() > remaining {
            return Err(QueryError::new(
                QueryErrorKind::ResourceExhausted,
                "evidence",
                "source verification budget exhausted",
            ));
        }
        if crate::utils::sha256_hex(&bytes) != reference.raw_digest {
            return Ok(None);
        }
        let formats =
            crate::domain::FileFormat::from_path(&self.lease.root.join(&reference.source_id))
                .map_err(|_| unavailable("source format unavailable"))?;
        let Some(decoded) = crate::utils::decode_index_text(&formats, &bytes) else {
            return Ok(None);
        };
        Ok(
            (crate::utils::sha256_hex(decoded.as_bytes()) == reference.decoded_digest)
                .then(|| decoded.into_owned()),
        )
    }
    fn read(&mut self, reference: &EvidenceRef) -> QueryResult<EvidenceRead> {
        self.context.check()?;
        reference.validate()?;
        let source = self.reader.source(&reference.source_id, self.context)?;
        if source.raw_digest.as_ref() != Some(&reference.raw_digest)
            || source.decoded_digest.as_ref() != Some(&reference.decoded_digest)
        {
            return Ok(EvidenceRead::Unavailable {
                reference: reference.clone(),
                reason: "source_version_mismatch".into(),
            });
        }
        let content = if let Some(text) = self.mapped(reference, &source)? {
            Some(text)
        } else {
            self.file(reference)?
                .map(|text| reference.span.slice(text).map(str::to_owned))
                .transpose()?
        };
        Ok(match content {
            Some(content) => EvidenceRead::Verified {
                reference: reference.clone(),
                content,
            },
            None => EvidenceRead::Unavailable {
                reference: reference.clone(),
                reason: "same_version_text_unavailable".into(),
            },
        })
    }
}
