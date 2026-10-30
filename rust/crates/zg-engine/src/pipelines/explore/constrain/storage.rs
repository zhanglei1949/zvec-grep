use super::{
    Content, ContextResult, EngineError, EngineResult, EntityMetadata, PreparedScope, kg,
    query_error,
};
use crate::{
    api::context::result::{ContentRange, ContextItem},
    domain::FileId,
    pipelines::indexed_search::storage::SearchStorage,
    storage::{
        IndexStore,
        types::{StorageSearchFilter, StorageSearchHit, StoredFileAttributes, StoredSearchData},
    },
};
use std::path::PathBuf;

pub(in crate::pipelines) struct ScopedStorage<'a> {
    inner: &'a IndexStore,
    scope: &'a PreparedScope,
    ctx: kg::RequestContext,
}
impl<'a> ScopedStorage<'a> {
    pub(super) fn new(
        inner: &'a IndexStore,
        scope: &'a PreparedScope,
        ctx: kg::RequestContext,
    ) -> Self {
        Self { inner, scope, ctx }
    }
    fn before(&self, filter: Option<&StorageSearchFilter>) -> EngineResult<()> {
        self.ctx.check().map_err(query_error)?;
        let actual = filter
            .and_then(|f| f.fragment_ids.as_ref())
            .ok_or_else(|| EngineError::internal("missing hard scope"))?;
        if actual
            .iter()
            .any(|id| !self.scope.units.iter().any(|u| &u.fragment_id == id))
        {
            return Err(EngineError::internal("scope widened before recall"));
        }
        self.ctx
            .charge(kg::BudgetUsage {
                filter_bytes: self.inner.search_filter_bytes(filter)? as u64,
                ..Default::default()
            })
            .map_err(query_error)
    }
    fn after(&self, hits: Vec<StorageSearchHit>) -> EngineResult<Vec<StorageSearchHit>> {
        self.ctx
            .charge(kg::BudgetUsage {
                candidates: hits.len() as u64,
                ..Default::default()
            })
            .map_err(query_error)?;
        for hit in &hits {
            if !self
                .scope
                .units
                .iter()
                .any(|u| u.fragment_id == hit.document_id && u.entity_id == hit.entity_id.as_str())
            {
                return Err(EngineError::storage_failure(
                    "native recall escaped hard scope",
                ));
            }
        }
        Ok(hits)
    }
}
impl SearchStorage for ScopedStorage<'_> {
    fn restrict_filter(
        &self,
        filter: Option<StorageSearchFilter>,
    ) -> EngineResult<Option<StorageSearchFilter>> {
        let mut filter = filter.unwrap_or_default();
        let allowed = self
            .scope
            .units
            .iter()
            .map(|u| u.fragment_id.clone())
            .filter(|id| {
                filter
                    .fragment_ids
                    .as_ref()
                    .is_none_or(|old| old.contains(id))
            })
            .collect();
        filter.fragment_ids = Some(allowed);
        Ok(Some(filter))
    }
    fn list_file_paths(&self) -> EngineResult<Vec<(FileId, PathBuf)>> {
        self.inner.list_file_paths()
    }
    fn list_file_attributes(&self) -> EngineResult<Vec<StoredFileAttributes>> {
        self.inner.list_file_attributes()
    }
    fn has_non_unicode_file_names(&self) -> EngineResult<bool> {
        self.inner.has_non_unicode_file_names()
    }
    fn load_search_hits(&self, hits: &[StorageSearchHit]) -> EngineResult<StoredSearchData> {
        self.after(hits.to_vec())?;
        let data = self.inner.load_search_hits(hits)?;
        for stored in data.entities.values() {
            if self.scope.entities.get(stored.entity.id.as_str()) != Some(&stored.entity) {
                return Err(EngineError::storage_failure(
                    "scope text changed during retrieval",
                ));
            }
        }
        Ok(data)
    }
    fn search_fts(
        &self,
        query: &str,
        limit: usize,
        filter: Option<&StorageSearchFilter>,
    ) -> EngineResult<Vec<StorageSearchHit>> {
        self.before(filter)?;
        self.after(self.inner.search_fts(query, limit, filter)?)
    }
    fn search_vector(
        &self,
        model: &str,
        vector: &[f32],
        limit: usize,
        filter: Option<&StorageSearchFilter>,
    ) -> EngineResult<Vec<StorageSearchHit>> {
        self.before(filter)?;
        self.after(self.inner.search_vector(model, vector, limit, filter)?)
    }
}

impl PreparedScope {
    pub(super) fn verify_output(&self, result: &mut ContextResult) -> EngineResult<()> {
        for item in result.items.iter_mut().chain(
            result
                .group_results
                .iter_mut()
                .flat_map(|g| g.items.iter_mut()),
        ) {
            self.verify_item(item)?;
        }
        Ok(())
    }
    fn verify_item(&self, item: &mut ContextItem) -> EngineResult<()> {
        let id = item
            .entity_id
            .as_ref()
            .ok_or_else(|| EngineError::storage_failure("scope result has no entity"))?;
        let entity = self
            .entities
            .get(id)
            .ok_or_else(|| EngineError::storage_failure("scope result entity missing"))?;
        let Content::Text(body) = &entity.content else {
            return Err(EngineError::unsupported("scope_non_text_body"));
        };
        let displayed = match item.content_range {
            ContentRange::Text {
                start_byte_offset,
                end_byte_offset,
                ..
            } => Some(kg::ByteSpan {
                start: start_byte_offset as u64,
                end: end_byte_offset as u64,
            }),
            ContentRange::Byte {
                start_offset,
                end_offset,
            } => Some(kg::ByteSpan {
                start: start_offset,
                end: end_offset,
            }),
            ContentRange::File => None,
        };
        let unit = self
            .units
            .iter()
            .find(|u| {
                &u.entity_id == id
                    && u.source_id == item.relative_path.to_string_lossy()
                    && displayed.is_none_or(|span| u.source_spans == [span])
                    && (displayed.is_some()
                        || (u.content_span.start == 0 && u.content_span.end == body.len() as u64))
                    && u.content_span
                        .slice(body)
                        .is_ok_and(|text| text == item.content)
            })
            .ok_or_else(|| {
                EngineError::storage_failure("scope result content escaped allowed units")
            })?;
        let span = unit.source_spans[0];
        if !matches!(item.content_range,ContentRange::Text{start_byte_offset,end_byte_offset,..} if start_byte_offset as u64==span.start && end_byte_offset as u64==span.end)
        {
            item.content_range = ContentRange::Byte {
                start_offset: span.start,
                end_offset: span.end,
            };
        }
        item.range = item.content_range.clone();
        item.excerpt_range = Some(item.content_range.clone());
        item.outline = None;
        item.container = None;
        if let Some(EntityMetadata::Code(code)) = &mut item.metadata {
            code.documentation = None;
            code.signature = None;
        }
        Ok(())
    }
}
