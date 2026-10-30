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
}
pub trait GraphReader {
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
