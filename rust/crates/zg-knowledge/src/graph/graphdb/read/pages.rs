//! Parameterized keyset adjacency. LIMIT bounds returned rows, not scan work.
use super::{
    Direction, GraphDbStore, QueryError, QueryResult, RelationKind, RelationRecord, RequestContext,
    ResolvedReference, error, invalid, params, strings, table, unjson,
};
use crate::{
    query::{EdgeKey, NeighborRead},
    semantic::{Identity, ObjectId},
};

fn identity(value: &str) -> QueryResult<Identity> {
    let (space_id, id) =
        serde_json::from_str(value).map_err(|_| QueryError::invalid("invalid stored identity"))?;
    Ok(Identity { space_id, id })
}
#[expect(
    clippy::too_many_lines,
    reason = "three fixed adjacency mappings share the same keyset contract"
)]
pub(super) fn read(
    store: &GraphDbStore,
    read: &NeighborRead<'_>,
    ctx: &RequestContext,
) -> QueryResult<Vec<RelationRecord>> {
    ctx.check()?;
    read.selector.validate()?;
    if read.boundary.sources.as_ref().is_some_and(Vec::is_empty)
        || read.boundary.kinds.as_ref().is_some_and(Vec::is_empty)
        || read
            .selector
            .relation_types
            .as_ref()
            .is_some_and(Vec::is_empty)
    {
        return Ok(Vec::new());
    }
    if let ObjectId::SemanticEntity { identity } | ObjectId::Episode { identity } = read.anchor
        && read
            .spaces
            .is_some_and(|spaces| !spaces.contains(&identity.space_id))
    {
        return Ok(Vec::new());
    }
    let key = read.after.cloned().unwrap_or_default();
    let allowed_sources = strings(read.boundary.sources.as_deref().unwrap_or(&[]));
    let allowed_kinds = strings(read.boundary.kinds.as_deref().unwrap_or(&[]));
    let no_sources = read.boundary.sources.is_none();
    let no_kinds = read.boundary.kinds.is_none();
    let kind = read.selector.edge_type;
    let all_contexts = read.contexts.is_none();
    let contexts = strings(read.contexts.unwrap_or(&[]));
    if !matches!(
        kind,
        RelationKind::SemanticRelation | RelationKind::EpisodeLink | RelationKind::Mentions
    ) {
        if read.skip_group {
            return Ok(Vec::new());
        }
        let ObjectId::SourceNode { node_id } = read.anchor else {
            return Ok(Vec::new());
        };
        let table = table(kind)?;
        let direction = match read.direction {
            Direction::Outgoing => "a.id=$anchor",
            Direction::Incoming => "b.id=$anchor",
            Direction::Both => "(a.id=$anchor OR b.id=$anchor)",
        };
        let step = match read.direction {
            Direction::Outgoing => "b.entity_kind IN $kinds",
            Direction::Incoming => "a.entity_kind IN $kinds",
            Direction::Both => {
                "((a.id=$anchor AND b.entity_kind IN $kinds) OR (b.id=$anchor AND a.entity_kind IN $kinds))"
            }
        };
        let last = if kind == RelationKind::Contains {
            ""
        } else {
            " OR (a.id=$source AND b.id=$target AND e.ref_id>$id)"
        };
        let columns = if kind == RelationKind::Contains {
            ""
        } else {
            ",e.ref_id,e.data"
        };
        let order = if kind == RelationKind::Contains {
            ""
        } else {
            ",e.ref_id"
        };
        let sql = format!(
            "MATCH (a:KgEntity)-[e:{table}]->(b:KgEntity) WHERE {direction} AND ($no_sources OR (a.file_path IN $sources AND b.file_path IN $sources)) AND ($no_kinds OR {step}) AND (a.id>$source OR (a.id=$source AND b.id>$target){last}) RETURN a.id,b.id{columns} ORDER BY a.id,b.id{order} LIMIT 256"
        );
        return store
            .with_read(|tx, options| {
                let mut parameters = vec![
                    ("anchor", node_id as &dyn ::graphdb::ToParam),
                    ("no_sources", &no_sources),
                    ("sources", &allowed_sources),
                    ("no_kinds", &no_kinds),
                    ("kinds", &allowed_kinds),
                    ("source", &key.source),
                    ("target", &key.target),
                ];
                if kind != RelationKind::Contains {
                    parameters.push(("id", &key.id));
                }
                let result = tx.query_with(&sql, ::graphdb::Params::named(parameters), options)?;
                let mut cursor = result.rows();
                let mut output = Vec::new();
                while let Some(row) = cursor.next()? {
                    let source: String = row.try_get(0)?;
                    let target: String = row.try_get(1)?;
                    let reference = if kind == RelationKind::Contains {
                        None
                    } else {
                        let stored: ResolvedReference = unjson(&row.try_get::<String>(3)?)?;
                        Some(
                            store
                                .reference_record(&stored.reference, Some(target.clone()), ctx)
                                .map_err(|_| invalid())?,
                        )
                    };
                    output.push(RelationRecord {
                        relation_type: None,
                        source_object: Some(ObjectId::SourceNode {
                            node_id: source.clone(),
                        }),
                        target_object: Some(ObjectId::SourceNode {
                            node_id: target.clone(),
                        }),
                        fact: None,
                        mention: None,
                        episode_link: None,
                        id: reference.as_ref().map_or_else(
                            || format!("contains:{source}:{target}"),
                            |r| r.id.clone(),
                        ),
                        source,
                        target,
                        kind,
                        reference,
                    });
                }
                Ok(output)
            })
            .map_err(|e| error(&e));
    }
    if kind == RelationKind::Mentions {
        return mentions(store, read, ctx, &key);
    }
    let (identity, node_table, table, type_column) = match (kind, read.anchor) {
        (RelationKind::SemanticRelation, ObjectId::SemanticEntity { identity }) => (
            identity,
            "KgSemanticEntity",
            "KgSemanticRelation",
            "predicate",
        ),
        (RelationKind::EpisodeLink, ObjectId::Episode { identity }) => {
            (identity, "KgEpisode", "KgEpisodeLink", "relation_type")
        }
        _ => return Ok(Vec::new()),
    };
    let anchor = identity
        .storage_key()
        .map_err(|_| QueryError::invalid("invalid anchor"))?;
    let direction = match read.direction {
        Direction::Outgoing => "a.id=$anchor",
        Direction::Incoming => "b.id=$anchor",
        Direction::Both => "(a.id=$anchor OR b.id=$anchor)",
    };
    let step = match read.direction {
        Direction::Outgoing => "b.kind IN $kinds",
        Direction::Incoming => "a.kind IN $kinds",
        Direction::Both => {
            "((a.id=$anchor AND b.kind IN $kinds) OR (b.id=$anchor AND a.kind IN $kinds))"
        }
    };
    let types = strings(read.selector.relation_types.as_deref().unwrap_or(&[]));
    let all_types = read.selector.relation_types.is_none();
    let current = if kind == RelationKind::SemanticRelation {
        "AND e.state='active' AND ($all_contexts OR e.context IN $contexts)"
    } else {
        "AND a.state='active' AND b.state='active' AND ($all_contexts OR (a.context IN $contexts AND b.context IN $contexts))"
    };
    let sql = format!(
        "MATCH (a:{node_table})-[e:{table}]->(b:{node_table}) WHERE {direction} AND e.space_id=$space {current} AND ($no_kinds OR {step}) AND ($all_types OR e.{type_column} IN $types) AND (e.{type_column}>$type OR (e.{type_column}=$type AND $continue_group AND (a.id>$source OR (a.id=$source AND b.id>$target) OR (a.id=$source AND b.id=$target AND e.id>$id)))) RETURN a.id,b.id,e.id,e.{type_column} ORDER BY e.{type_column},a.id,b.id,e.id LIMIT 256"
    );
    store.with_read(|tx,options| {
        let result=tx.query_with(&sql,params!{"anchor"=>&anchor,"space"=>&identity.space_id,"all_contexts"=>all_contexts,"contexts"=>&contexts,"no_kinds"=>no_kinds,"kinds"=>&allowed_kinds,"all_types"=>all_types,"types"=>&types,"type"=>&key.relation_type,"continue_group"=>!read.skip_group,"source"=>&key.source,"target"=>&key.target,"id"=>&key.id},options)?;
        let mut cursor=result.rows();let mut output=Vec::new();
        while let Some(row)=cursor.next()? {
            let source:String=row.try_get(0)?;let target:String=row.try_get(1)?;let id:String=row.try_get(2)?;let relation_type:String=row.try_get(3)?;
            let a=super::pages::identity(&source).map_err(|_|invalid())?;let b=super::pages::identity(&target).map_err(|_|invalid())?;
            output.push(RelationRecord {relation_type:Some(relation_type),source_object:Some(if kind==RelationKind::EpisodeLink {ObjectId::Episode{identity:a}}else{ObjectId::SemanticEntity{identity:a}}),target_object:Some(if kind==RelationKind::EpisodeLink {ObjectId::Episode{identity:b}}else{ObjectId::SemanticEntity{identity:b}}),fact:None,mention:None,episode_link:None,id,source,target,kind,reference:None});
        }
        Ok(output)
    }).map_err(|e|error(&e))
}

fn mentions(
    store: &GraphDbStore,
    read: &NeighborRead<'_>,
    ctx: &RequestContext,
    key: &EdgeKey,
) -> QueryResult<Vec<RelationRecord>> {
    ctx.check()?;
    if read.skip_group {
        return Ok(Vec::new());
    }
    let (anchor, predicate) = match read.anchor {
        ObjectId::SourceNode { node_id } if read.direction != Direction::Incoming => {
            (node_id.clone(), "a.id=$anchor")
        }
        ObjectId::SemanticEntity { identity } if read.direction != Direction::Outgoing => (
            identity
                .storage_key()
                .map_err(|_| QueryError::invalid("invalid anchor"))?,
            "b.id=$anchor",
        ),
        _ => return Ok(Vec::new()),
    };
    let sources = strings(read.boundary.sources.as_deref().unwrap_or(&[]));
    let kinds = strings(read.boundary.kinds.as_deref().unwrap_or(&[]));
    let spaces = strings(read.spaces.unwrap_or(&[]));
    let destination = if matches!(read.anchor, ObjectId::SourceNode { .. }) {
        "b.kind IN $kinds"
    } else {
        "a.entity_kind IN $kinds"
    };
    store.with_read(|tx,options| {
        let sql=format!("MATCH (a:KgEntity)-[e:KgMentions]->(b:KgSemanticEntity) WHERE {predicate} AND ($all_sources OR a.file_path IN $sources) AND ($all_spaces OR b.space_id IN $spaces) AND ($all_kinds OR {destination}) AND (a.id>$source OR (a.id=$source AND b.id>$target) OR (a.id=$source AND b.id=$target AND e.ref_id>$id)) RETURN a.id,b.id,e.ref_id,e.data ORDER BY a.id,b.id,e.ref_id LIMIT 256");
        let result=tx.query_with(&sql,params!{"anchor"=>&anchor,"all_sources"=>read.boundary.sources.is_none(),"sources"=>&sources,"all_spaces"=>read.spaces.is_none(),"spaces"=>&spaces,"all_kinds"=>read.boundary.kinds.is_none(),"kinds"=>&kinds,"source"=>&key.source,"target"=>&key.target,"id"=>&key.id},options)?;
        let mut cursor=result.rows();let mut output=Vec::new();
        while let Some(row)=cursor.next()? {
            let source:String=row.try_get(0)?;let target:String=row.try_get(1)?;
            let mention=unjson::<crate::semantic::Mention>(&row.try_get::<String>(3)?)?;
            output.push(RelationRecord {relation_type:None,source_object:Some(ObjectId::SourceNode{node_id:source.clone()}),target_object:Some(ObjectId::SemanticEntity{identity:identity(&target).map_err(|_|invalid())?}),fact:None,mention:Some(mention),episode_link:None,id:row.try_get(2)?,source,target,kind:RelationKind::Mentions,reference:None});
        }
        Ok(output)
    }).map_err(|e|error(&e))
}
