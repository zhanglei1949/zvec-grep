//! Business graph operations over static, parameterized native statements.
mod pages;
use super::{
    GraphDbStore,
    codec::{ResolvedReference, unjson, untag},
};
use crate::{
    graph::{
        GraphError, GraphErrorKind, GraphResult,
        schema::{FileRow, NodeRow},
    },
    processing::ProcessingContext,
    query::{
        ByteSpan, Direction, EntityRecord, EvidenceRef, GraphReader, Locator, QueryError,
        QueryErrorKind, QueryResult, ReferencePartition, ReferenceRecord, RelationKind,
        RelationRecord, RequestContext, SourceRecord,
    },
};
use ::graphdb::{LogicalType, Value, params};

fn error(e: &GraphError) -> QueryError {
    let kind = match e.kind {
        GraphErrorKind::Cancelled => QueryErrorKind::Cancelled,
        GraphErrorKind::Deadline => QueryErrorKind::StageDeadline,
        GraphErrorKind::ResourceExhausted => QueryErrorKind::ResourceExhausted,
        GraphErrorKind::RecoveryRequired => QueryErrorKind::RecoveryRequired,
        GraphErrorKind::IncompatibleFormat => QueryErrorKind::GraphUnavailable,
        _ => QueryErrorKind::Internal,
    };
    QueryError::new(kind, "graph_read", "native graph read failed").with_detail(format!("{e:?}"))
}
fn invalid() -> GraphError {
    GraphError::local(GraphErrorKind::IncompatibleFormat)
}
fn strings(ids: &[String]) -> Value {
    Value::List {
        element_type: LogicalType::String,
        values: ids
            .iter()
            .cloned()
            .map(|s| Value::String(s.into()))
            .collect(),
    }
}
fn entity(row: NodeRow) -> GraphResult<EntityRecord> {
    Ok(EntityRecord {
        id: row.id,
        source_id: row.file_path.clone(),
        kind: untag(&row.entity_kind)?,
        name: if row.name.is_empty() {
            row.file_path
        } else {
            row.name
        },
        qualified_name: row.qualified_label,
        span: ByteSpan {
            start: row.span_start,
            end: row.span_end,
        },
    })
}
fn source(row: FileRow) -> GraphResult<SourceRecord> {
    Ok(SourceRecord {
        record: crate::relations::bound::FileRecord {
            path: row.path,
            file_id: row.file_id.try_into().map_err(|_| invalid())?,
            index_status: untag(&row.index_status)?,
            content_hash: row.content_hash,
            size_bytes: row.size_bytes,
            text_revision: row.text_revision,
            storage_generation: row.storage_generation,
            entity_count: row.entity_count,
            entity_ids_digest: row.entity_ids_digest,
        },
        language: row.language.as_deref().map(untag).transpose()?,
        complete: row.complete.unwrap_or(false),
        raw_digest: row.raw_digest,
        decoded_digest: row.decoded_digest,
        text_map: unjson(&row.text_map)?,
        text_binding_digest: row.text_binding_digest,
    })
}
fn table(kind: RelationKind) -> QueryResult<&'static str> {
    match kind {
        RelationKind::Contains => Ok("KgContains"),
        RelationKind::Calls => Ok("KgCalls"),
        RelationKind::Imports => Ok("KgImports"),
        RelationKind::Extends => Ok("KgExtends"),
        RelationKind::Implements => Ok("KgImplements"),
        RelationKind::Links
        | RelationKind::Mentions
        | RelationKind::SemanticRelation
        | RelationKind::EpisodeLink => Err(QueryError::new(
            QueryErrorKind::Unsupported,
            "graph_read",
            "relation unsupported in code explore",
        )),
    }
}
impl GraphDbStore {
    fn implementation_references(
        &self,
        owner: Option<&str>,
        source: &str,
        id: Option<&str>,
        offset: u64,
        context: &RequestContext,
    ) -> QueryResult<Vec<ReferenceRecord>> {
        // v7 edges retain the declaration file in the typed payload. Its owner
        // can be declared elsewhere; page by declaration rather than owner file.
        let mut raw_offset = 0u64;
        let mut matched = 0u64;
        let mut output = Vec::new();
        loop {
            context.check()?;
            let rows = self.with_read(|tx, options| {
                let result = tx.query_with(&format!("MATCH (a:KgEntity)-[e:KgImplements]->(b:KgEntity) WHERE ($owner='' OR a.id=$owner) AND ($id='' OR e.ref_id=$id) RETURN e.data,b.id ORDER BY e.ref_id SKIP {raw_offset} LIMIT 256"), params!{"owner"=>owner.unwrap_or(""),"id"=>id.unwrap_or("")}, options)?;
                let mut cursor = result.rows(); let mut rows = Vec::new();
                while let Some(row) = cursor.next()? { rows.push((unjson::<ResolvedReference>(&row.try_get::<String>(0)?)?, row.try_get::<String>(1)?)); }
                Ok(rows)
            }).map_err(|e| error(&e))?;
            context.charge(crate::query::BudgetUsage {
                candidates: rows.len() as u64,
                ..Default::default()
            })?;
            let count = rows.len();
            for (mut resolved, target) in rows {
                if resolved.reference.file_path != source {
                    continue;
                }
                if matched >= offset {
                    if let Some(source) = resolved.relation_source {
                        resolved.reference.owner_id = source.node_id;
                    }
                    output.push(self.reference_record(
                        &resolved.reference,
                        Some(target),
                        context,
                    )?);
                    if output.len() == 256 {
                        return Ok(output);
                    }
                }
                matched += 1;
            }
            if count < 256 {
                return Ok(output);
            }
            raw_offset += 256;
        }
    }
    pub fn find_source(
        &self,
        id: &str,
        context: &RequestContext,
    ) -> QueryResult<Option<SourceRecord>> {
        context.check()?;
        self.with_read(|tx, options| {
            let rows = FileRow::select(
                tx,
                options,
                "WHERE n.path=$source",
                params! {"source"=>id},
                0,
            )?;
            rows.into_iter().map(source).next().transpose()
        })
        .map_err(|e| error(&e))
    }
    pub fn source_records(
        &self,
        after: &str,
        context: &ProcessingContext,
    ) -> GraphResult<Vec<SourceRecord>> {
        context.check()?;
        self.with_read(|tx, options| {
            FileRow::page(tx, options, after)?
                .into_iter()
                .map(source)
                .collect()
        })
    }
    fn reference_record(
        &self,
        row: &crate::graph::schema::ReferenceRow,
        target: Option<String>,
        context: &RequestContext,
    ) -> QueryResult<ReferenceRecord> {
        let src = self.source(&row.file_path, context)?;
        let kind = match row.kind.as_str() {
            "call" => RelationKind::Calls,
            "import" => RelationKind::Imports,
            "document_link" => RelationKind::Links,
            "extends" => RelationKind::Extends,
            "implements" => RelationKind::Implements,
            _ => return Err(QueryError::invalid("unknown persisted reference kind")),
        };
        Ok(ReferenceRecord {
            id: row.id.clone(),
            owner: row.owner_id.clone(),
            target,
            kind,
            status: row
                .status
                .as_deref()
                .map(untag)
                .transpose()
                .map_err(|e| error(&e))?
                .unwrap_or(crate::relations::ResolutionStatus::Unresolved),
            reason: row
                .reason
                .clone()
                .unwrap_or_else(|| "resolution_pending".into()),
            locator: unjson(&row.lookup).map_err(|e| error(&e))?,
            evidence: EvidenceRef {
                source_id: row.file_path.clone(),
                raw_digest: src
                    .raw_digest
                    .ok_or_else(|| QueryError::invalid("reference source digest missing"))?,
                decoded_digest: src
                    .decoded_digest
                    .ok_or_else(|| QueryError::invalid("reference decoded digest missing"))?,
                span: ByteSpan {
                    start: row.span_start,
                    end: row.span_end,
                },
                reference_id: Some(row.id.clone()),
            },
        })
    }
}
impl GraphReader for GraphDbStore {
    fn neighbors_after(
        &self,
        read: &crate::query::NeighborRead<'_>,
        ctx: &RequestContext,
    ) -> QueryResult<Vec<RelationRecord>> {
        pages::read(self, read, ctx)
    }
    fn semantic_selection(
        &self,
        query: &crate::query::SemanticQuery,
        context: &RequestContext,
    ) -> QueryResult<Option<crate::semantic::SemanticCatalog>> {
        self.with_read(|tx, options| {
            super::codec::FactRows::select_semantic(tx, options, query, context, false)
        })
        .map(Some)
        .map_err(|e| error(&e))
    }

    fn semantic_object(
        &self,
        query: &crate::query::SemanticQuery,
        context: &RequestContext,
    ) -> QueryResult<Option<crate::semantic::SemanticCatalog>> {
        if query.anchor.is_none() {
            return Err(QueryError::invalid(
                "exact semantic object requires an anchor",
            ));
        }
        self.with_read(|tx, options| {
            super::codec::FactRows::select_semantic(tx, options, query, context, true)
        })
        .map(Some)
        .map_err(|e| error(&e))
    }
    fn source_nodes(
        &self,
        ids: &[String],
        context: &RequestContext,
    ) -> QueryResult<Vec<EntityRecord>> {
        context.check()?;
        if ids.len() > 256 {
            return Err(QueryError::invalid("source node batch exceeds 256"));
        }
        self.with_read(|tx, options| {
            NodeRow::select(
                tx,
                options,
                "WHERE n.id IN $ids",
                params! {"ids"=>&strings(ids)},
                0,
            )?
            .into_iter()
            .map(entity)
            .collect()
        })
        .map_err(|e| error(&e))
    }
    fn semantic_page(
        &self,
        partition: crate::query::SemanticPartition,
        space: &str,
        offset: u64,
        context: &RequestContext,
    ) -> QueryResult<crate::semantic::SemanticCatalog> {
        context.check()?;
        self.with_read(|tx, options| {
            super::codec::FactRows::semantic_page(tx, options, partition, space, offset)
        })
        .map_err(|e| error(&e))
    }
    fn span_nodes(
        &self,
        source: &str,
        span: ByteSpan,
        offset: u64,
        context: &RequestContext,
    ) -> QueryResult<Vec<EntityRecord>> {
        context.check()?;
        span.validate()?;
        if span.start == span.end {
            return Err(QueryError::invalid("span lookup requires text"));
        }
        self.with_read(|tx, options| {
            NodeRow::select(
                tx,
                options,
                "WHERE n.domain='code' AND n.file_path=$source AND n.span_start < $upper_bound AND n.span_end > $lower_bound",
                params! {"source"=>source,"lower_bound"=>span.start,"upper_bound"=>span.end},
                offset,
            )?
            .into_iter()
            .map(entity)
            .collect()
        }).map_err(|e| error(&e))
    }
    fn source(&self, id: &str, context: &RequestContext) -> QueryResult<SourceRecord> {
        context.check()?;
        self.with_read(|tx, options| {
            let mut rows = FileRow::select(
                tx,
                options,
                "WHERE n.path=$source",
                params! {"source"=>id},
                0u64,
            )?;
            if rows.len() != 1 {
                return Err(invalid());
            }
            source(rows.remove(0))
        })
        .map_err(|e| error(&e))
    }
    fn locate(
        &self,
        locator: &Locator,
        offset: u64,
        context: &RequestContext,
    ) -> QueryResult<Vec<EntityRecord>> {
        context.check()?;
        self.with_read(|tx,options| {
            let rows = match locator {
                Locator::Object{object}=>match &object.object {crate::semantic::ObjectId::SourceNode{node_id}=>NodeRow::select(tx,options,"WHERE n.id=$id",params!{"id"=>node_id},offset)?,_=>return Err(invalid())},
                Locator::Node{node}=>NodeRow::select(tx,options,"WHERE n.domain='code' AND n.id=$id",params!{"id"=>&node.node_id},offset)?,
                Locator::Name{name,source_id,kind}=> {
                    let kind=kind.map(|k|serde_json::to_value(k).map(|v|v.as_str().unwrap_or_default().to_owned())).transpose().map_err(|_|invalid())?.unwrap_or_default();
                    NodeRow::select(tx,options,"WHERE n.domain='code' AND (n.name=$name OR n.qualified_label=$name OR (n.kind='file' AND n.file_path=$name)) AND ($source='' OR n.file_path=$source) AND ($kind='' OR n.entity_kind=$kind)",params!{"name"=>name,"source"=>source_id.as_deref().unwrap_or(""),"kind"=>&kind},offset)?
                },
                Locator::Position{source_id,byte_offset,kind}=>{
                    let kind=kind.map(|k|serde_json::to_value(k).map(|v|v.as_str().unwrap_or_default().to_owned())).transpose().map_err(|_|invalid())?.unwrap_or_default();
                    NodeRow::select(tx,options,"WHERE n.domain='code' AND n.file_path=$source AND n.span_start <= $position AND (n.span_end > $position OR (n.span_start=$position AND n.span_end=$position)) AND ($kind='' OR n.entity_kind=$kind)",params!{"source"=>source_id,"position"=>byte_offset,"kind"=>&kind},offset)?
                }
            }; rows.into_iter().map(entity).collect()
        }).map_err(|e|error(&e))
    }
    fn get_nodes(
        &self,
        ids: &[String],
        context: &RequestContext,
    ) -> QueryResult<Vec<EntityRecord>> {
        context.check()?;
        if ids.len() > 256 {
            return Err(QueryError::invalid("node batch exceeds 256"));
        }
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        self.with_read(|tx, options| {
            NodeRow::select(
                tx,
                options,
                "WHERE n.domain='code' AND n.id IN $ids",
                params! {"ids"=>&strings(ids)},
                0u64,
            )?
            .into_iter()
            .map(entity)
            .collect()
        })
        .map_err(|e| error(&e))
    }
    fn neighbors(
        &self,
        ids: &[String],
        kind: RelationKind,
        direction: Direction,
        offset: u64,
        context: &RequestContext,
    ) -> QueryResult<Vec<RelationRecord>> {
        context.check()?;
        if ids.len() > 256 {
            return Err(QueryError::invalid("frontier batch exceeds 256"));
        }
        let table = table(kind)?;
        let predicate = match direction {
            Direction::Outgoing => "a.id IN $ids",
            Direction::Incoming => "b.id IN $ids",
            Direction::Both => "a.id IN $ids OR b.id IN $ids",
        };
        let extra = if kind == RelationKind::Contains {
            ""
        } else {
            ",e.data"
        };
        let order = if extra.is_empty() { "" } else { ",e.ref_id" };
        let raw=self.with_read(|tx,options| {
            let result=tx.query_with(&format!("MATCH (a:KgEntity)-[e:{table}]->(b:KgEntity) WHERE {predicate} RETURN a.id,b.id{extra} ORDER BY a.id,b.id{order} SKIP {offset} LIMIT 256"),params!{"ids"=>&strings(ids)},options)?;
            let mut cursor=result.rows();let mut rows=Vec::new();
            while let Some(row)=cursor.next()? {rows.push((row.try_get::<String>(0)?,row.try_get::<String>(1)?,if extra.is_empty(){None}else{Some(row.try_get::<String>(2)?)}));} Ok(rows)
        }).map_err(|e|error(&e))?;
        raw.into_iter()
            .map(|(source, target, data)| {
                let reference = data
                    .map(|v| unjson::<ResolvedReference>(&v).map_err(|e| error(&e)))
                    .transpose()?
                    .map(|mut r| {
                        r.reference.owner_id.clone_from(&source);
                        self.reference_record(&r.reference, Some(target.clone()), context)
                    })
                    .transpose()?;
                Ok(RelationRecord {
                    mention: None,
                    episode_link: None,
                    relation_type: None,
                    source_object: None,
                    target_object: None,
                    fact: None,
                    id: reference
                        .as_ref()
                        .map_or_else(|| format!("contains:{source}:{target}"), |r| r.id.clone()),
                    source,
                    target,
                    kind,
                    reference,
                })
            })
            .collect()
    }
    fn references(
        &self,
        partition: ReferencePartition,
        owner: Option<&str>,
        source_id: Option<&str>,
        id: Option<&str>,
        offset: u64,
        context: &RequestContext,
    ) -> QueryResult<Vec<ReferenceRecord>> {
        context.check()?;
        if partition == ReferencePartition::Implements
            && let Some(source) = source_id
        {
            return self.implementation_references(owner, source, id, offset, context);
        }
        // Union before paging: the same ref_id occurs in exactly one branch.
        let raw=self.with_read(|tx,options| {
            let mut result_rows=Vec::new();
            let name=match partition {ReferencePartition::Unresolved=>"KgUnresolvedRef",ReferencePartition::Calls=>"KgCalls",ReferencePartition::Imports=>"KgImports",ReferencePartition::Extends=>"KgExtends",ReferencePartition::Implements=>"KgImplements"};
            {
                let (q, data)=if name=="KgUnresolvedRef" {
                    (format!("MATCH (r:KgUnresolvedRef) WHERE r.kind <> 'document_link' AND ($owner='' OR r.owner_id=$owner) AND ($source='' OR r.file_path=$source) AND ($id='' OR r.id=$id) RETURN r.id,r.file_path,r.owner_id,r.kind,r.status,r.reason,r.span_start,r.span_end,r.lookup ORDER BY r.id SKIP {offset} LIMIT 256"),false)
                } else { (format!("MATCH (a:KgEntity)-[e:{name}]->(b:KgEntity) WHERE ($owner='' OR a.id=$owner) AND ($source='' OR a.file_path=$source) AND ($id='' OR e.ref_id=$id) RETURN e.data,b.id ORDER BY e.ref_id SKIP {offset} LIMIT 256"),true) };
                let result=tx.query_with(&q,params!{"owner"=>owner.unwrap_or(""),"source"=>source_id.unwrap_or(""),"id"=>id.unwrap_or("")},options)?;
                let mut cursor=result.rows();
                while let Some(row)=cursor.next()? {
                    if data { let mut r:ResolvedReference=unjson(&row.try_get::<String>(0)?)?;if let Some(source) = r.relation_source { r.reference.owner_id = source.node_id; }result_rows.push((r.reference,Some(row.try_get(1)?))); }
                    else {result_rows.push((crate::graph::schema::ReferenceRow { id:row.try_get(0)?,file_path:row.try_get(1)?,owner_id:row.try_get(2)?,kind:row.try_get(3)?,status:row.try_get(4)?,reason:row.try_get(5)?,span_start:row.try_get(6)?,span_end:row.try_get(7)?,lookup:row.try_get(8)?,ordinal:0,scope_ordinal:0,entity_ids:String::new() },None));}
                }
            } Ok(result_rows)
        }).map_err(|e|error(&e))?;
        raw.into_iter()
            .map(|(r, t)| self.reference_record(&r, t, context))
            .collect()
    }
}
