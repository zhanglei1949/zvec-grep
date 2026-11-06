//! Typed graph reads; each page contains at most 256 records.
use super::{
    ByteSpan, Direction, EntityRecord, Locator, QueryResult, ReferenceRecord, RelationKind,
    RelationRecord, RequestContext, TextMap,
};
#[derive(Clone, Debug)]
pub struct SourceRecord {
    pub record: crate::relations::bound::FileRecord,
    pub language: Option<crate::relations::Language>,
    pub complete: bool,
    pub raw_digest: Option<String>,
    pub decoded_digest: Option<String>,
    pub text_map: Vec<TextMap>,
    pub text_binding_digest: Option<String>,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReferencePartition {
    Unresolved,
    Calls,
    Imports,
    Extends,
    Implements,
}
pub trait GraphReader {
    /// Stable filtered edge pages. Native adapters override this to use keyset queries.
    fn neighbors_after(
        &self,
        read: &super::NeighborRead<'_>,
        context: &RequestContext,
    ) -> QueryResult<Vec<RelationRecord>> {
        let crate::semantic::ObjectId::SourceNode { node_id } = read.anchor else {
            return Err(super::QueryError::new(
                super::QueryErrorKind::Unsupported,
                "neighbors",
                "semantic adjacency unavailable",
            ));
        };
        let mut offset = 0;
        let mut selected = Vec::new();
        loop {
            context.check()?;
            let page = self.neighbors(
                std::slice::from_ref(node_id),
                read.selector.edge_type,
                read.direction,
                offset,
                context,
            )?;
            let count = page.len();
            for edge in page {
                let key = edge.key();
                if read.after.is_none_or(|after| key > *after)
                    && (!read.skip_group
                        || read
                            .after
                            .is_none_or(|after| key.relation_type > after.relation_type))
                {
                    selected.push(edge);
                }
            }
            if count < 256 {
                break;
            }
            offset += 256;
            context.charge(super::BudgetUsage {
                candidates: count as u64,
                ..Default::default()
            })?;
        }
        selected.sort_by_key(RelationRecord::key);
        selected.truncate(256);
        Ok(selected)
    }
    /// A backend may select candidates and their provenance directly. Test/read adapters
    /// without predicate support retain the bounded partition fallback.
    fn semantic_selection(
        &self,
        _query: &super::SemanticQuery,
        _context: &RequestContext,
    ) -> QueryResult<Option<crate::semantic::SemanticCatalog>> {
        Ok(None)
    }
    /// Exact endpoint metadata and support, without exploring its incident facts.
    fn semantic_object(
        &self,
        query: &super::SemanticQuery,
        context: &RequestContext,
    ) -> QueryResult<Option<crate::semantic::SemanticCatalog>> {
        self.semantic_selection(query, context)
    }
    fn source_nodes(
        &self,
        ids: &[String],
        context: &RequestContext,
    ) -> QueryResult<Vec<EntityRecord>> {
        self.get_nodes(ids, context)
    }
    /// Code-only readers explicitly lack this capability; views must not advertise it.
    fn semantic_page(
        &self,
        _partition: super::SemanticPartition,
        _space: &str,
        _offset: u64,
        _context: &RequestContext,
    ) -> QueryResult<crate::semantic::SemanticCatalog> {
        Err(super::QueryError::new(
            super::QueryErrorKind::Unsupported,
            "semantic_read",
            "reader has no semantic capability",
        ))
    }
    /// Business nodes overlapping a nonempty source interval, in stable ID order.
    /// Callers page to completion before deciding whether a hit has one owner.
    fn span_nodes(
        &self,
        source: &str,
        span: ByteSpan,
        offset: u64,
        context: &RequestContext,
    ) -> QueryResult<Vec<EntityRecord>>;
    fn locate(
        &self,
        locator: &Locator,
        offset: u64,
        context: &RequestContext,
    ) -> QueryResult<Vec<EntityRecord>>;
    fn get_nodes(&self, ids: &[String], context: &RequestContext)
    -> QueryResult<Vec<EntityRecord>>;
    fn neighbors(
        &self,
        ids: &[String],
        kind: RelationKind,
        direction: Direction,
        offset: u64,
        context: &RequestContext,
    ) -> QueryResult<Vec<RelationRecord>>;
    fn references(
        &self,
        partition: ReferencePartition,
        owner: Option<&str>,
        source: Option<&str>,
        id: Option<&str>,
        offset: u64,
        context: &RequestContext,
    ) -> QueryResult<Vec<ReferenceRecord>>;
    fn source(&self, source: &str, context: &RequestContext) -> QueryResult<SourceRecord>;
}
