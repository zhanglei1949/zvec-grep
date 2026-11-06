//! Predicate-qualified candidates, followed by only their required provenance.
use super::{
    AssociationRow, EpisodeRow, FactRows, GraphResult, Identity, ObjectId, ScalarFactRow,
    SemanticCatalog, SemanticChangeRow, SemanticEntityRow, SemanticRelationRow, SemanticSourceRow,
    parse,
};
use crate::query::{BudgetUsage, Direction, RequestContext, SemanticQuery};
use std::collections::BTreeSet;

fn strings(values: &[String]) -> ::graphdb::Value {
    ::graphdb::Value::List {
        element_type: ::graphdb::LogicalType::String,
        values: values
            .iter()
            .map(|v| ::graphdb::Value::String(v.as_str().into()))
            .collect(),
    }
}

fn charge(ctx: &RequestContext, usage: BudgetUsage) -> GraphResult<()> {
    ctx.check().and_then(|()| ctx.charge(usage)).map_err(|e| {
        crate::graph::GraphError::local(match e.kind {
            crate::query::QueryErrorKind::Cancelled => crate::graph::GraphErrorKind::Cancelled,
            crate::query::QueryErrorKind::RequestDeadline
            | crate::query::QueryErrorKind::StageDeadline => crate::graph::GraphErrorKind::Deadline,
            _ => crate::graph::GraphErrorKind::ResourceExhausted,
        })
    })
}

impl FactRows {
    #[expect(
        clippy::too_many_lines,
        reason = "static parameterized selection and targeted provenance within one read transaction"
    )]
    pub(in crate::graph::graphdb) fn select_semantic(
        tx: &mut ::graphdb::Transaction<'_>,
        options: &::graphdb::QueryOptions,
        query: &SemanticQuery,
        ctx: &RequestContext,
        endpoint_only: bool,
    ) -> GraphResult<SemanticCatalog> {
        let mut out = Self::default();
        let mut working_bytes = 0u64;
        macro_rules! retain_bytes {
            ($value:expr) => {
                ctx.reserve_working_set(&mut working_bytes, $value)
                    .map_err(|_| {
                        crate::graph::GraphError::local(
                            crate::graph::GraphErrorKind::ResourceExhausted,
                        )
                    })?
            };
        }
        let space = &query.space_id;
        let kind = query.kind.as_deref().unwrap_or("");
        let name = query.name.as_deref().unwrap_or("");
        let context = query.context.as_deref().unwrap_or("");
        let predicate = query.relation_type.as_deref().unwrap_or("");
        let anchor = query.anchor.as_ref().map(|a| &a.object);
        let key = match anchor {
            Some(
                ObjectId::SemanticEntity { identity }
                | ObjectId::Fact { identity }
                | ObjectId::Episode { identity },
            ) => identity.storage_key()?,
            Some(ObjectId::SourceNode { node_id }) => node_id.clone(),
            None => String::new(),
        };
        let anchor_type = match anchor {
            Some(ObjectId::SemanticEntity { .. }) => "entity",
            Some(ObjectId::Fact { .. }) => "fact",
            Some(ObjectId::Episode { .. }) => "episode",
            Some(ObjectId::SourceNode { .. }) => "source",
            None => "",
        };
        let outgoing = query.direction != Direction::Incoming;
        let incoming = query.direction != Direction::Outgoing;
        let history = query.history;
        let mut after = String::new();
        macro_rules! node_pages {
            ($row:ident,$field:ident,$clause:expr,$params:expr,$resource:ident) => {{
                after.clear();
                loop {
                    charge(ctx, BudgetUsage::default())?;
                    let page = $row::select_filtered(tx, options, $clause, $params)?;
                    let count = page.len();
                    charge(
                        ctx,
                        BudgetUsage {
                            $resource: count as u64,
                            ..Default::default()
                        },
                    )?;
                    if let Some(last) = page.last() {
                        after = last.key();
                    }
                    retain_bytes!(&page);
                    out.$field.extend(page);
                    if count < 256 {
                        break;
                    }
                }
            }};
        }
        // Seed discovery is independent of source support. Authorization is applied
        // to contributions before any selected entity/fact becomes visible.
        if anchor_type == "entity"
            || (anchor_type.is_empty() && (predicate.is_empty() || !name.is_empty()))
        {
            node_pages!(
                SemanticEntityRow,
                semantic_entities,
                "WHERE n.space_id=$space AND n.id>$after AND ($kind='' OR n.kind=$kind) AND ($anchor='' OR n.id=$anchor)",
                ::graphdb::params! {"space"=>space,"after"=>&after,"kind"=>kind,"anchor"=>&key},
                nodes
            );
        }
        // graphdb does not currently lower string CONTAINS. Inspect only the
        // kind-qualified entity candidates for exact aliases, then push their IDs
        // into relation reads. No auxiliary identity/type index is introduced.
        out.semantic_entities.retain(|e| {
            name.is_empty()
                || e.name == name
                || parse::<Vec<String>>(&e.aliases_json)
                    .is_ok_and(|aliases| aliases.iter().any(|alias| alias == name))
        });
        let named = strings(
            &out.semantic_entities
                .iter()
                .map(|e| e.id.clone())
                .collect::<Vec<_>>(),
        );
        if anchor_type != "episode" && !endpoint_only {
            let mut mentioned = BTreeSet::new();
            if anchor_type == "source" {
                let result=tx.query_with("MATCH (a:KgEntity)-[e:KgMentions]->(b:KgSemanticEntity) WHERE a.id=$id AND b.space_id=$space RETURN b.id",::graphdb::params!{"id"=>&key,"space"=>space},options)?;
                let mut cursor = result.rows();
                while let Some(row) = cursor.next()? {
                    let identity = row.try_get::<String>(0)?;
                    charge(
                        ctx,
                        BudgetUsage {
                            edges: 1,
                            ..Default::default()
                        },
                    )?;
                    retain_bytes!(&identity);
                    mentioned.insert(identity);
                }
            }
            let mentioned = mentioned.into_iter().collect::<Vec<_>>();
            after.clear();
            loop {
                charge(ctx, BudgetUsage::default())?;
                let result=tx.query_with("MATCH (a:KgSemanticEntity)-[e:KgSemanticRelation]->(b:KgSemanticEntity) WHERE e.space_id=$space AND e.id>$after AND ($history OR e.state='active') AND ($predicate='' OR e.predicate=$predicate) AND ($context='' OR e.context=$context) AND (($type='' AND ($kind='' OR a.kind=$kind OR b.kind=$kind) AND ($name='' OR a.id IN $named OR b.id IN $named)) OR ($type='fact' AND e.id=$anchor) OR ($type='entity' AND (($outgoing AND a.id=$anchor) OR ($incoming AND b.id=$anchor))) OR ($type='source' AND (a.id IN $mentioned OR b.id IN $mentioned))) RETURN a.id AS source,b.id AS target,e.id AS id,e.fact_id AS fact_id,e.space_id AS space_id,e.predicate AS predicate,e.context AS context,e.state AS state,e.supersedes AS supersedes,e.changed_at AS changed_at,e.created_at AS created_at,e.attributes_json AS attributes_json ORDER BY e.id LIMIT 256",::graphdb::params!{"space"=>space,"after"=>&after,"history"=>history,"predicate"=>predicate,"context"=>context,"type"=>anchor_type,"kind"=>kind,"name"=>name,"named"=>&named,"anchor"=>&key,"outgoing"=>outgoing,"incoming"=>incoming,"mentioned"=>&strings(&mentioned)},options)?;
                let mut cursor = result.rows();
                let mut page = Vec::new();
                while let Some(row) = cursor.next()? {
                    page.push(SemanticRelationRow::decode_row(&row)?);
                }
                charge(
                    ctx,
                    BudgetUsage {
                        edges: page.len() as u64,
                        ..Default::default()
                    },
                )?;
                let count = page.len();
                if let Some(last) = page.last() {
                    after.clone_from(&last.id);
                }
                retain_bytes!(&page);
                out.semantic_relations.extend(page);
                if count < 256 {
                    break;
                }
            }
            // Scalar facts use the same subject conditions. They remain auxiliary
            // fact records and do not acquire business graph endpoints.
            after.clear();
            loop {
                charge(ctx, BudgetUsage::default())?;
                let fields = "n.id AS id,n.fact_id AS fact_id,n.space_id AS space_id,n.subject AS subject,n.predicate AS predicate,n.object_json AS object_json,n.context AS context,n.state AS state,n.supersedes AS supersedes,n.changed_at AS changed_at,n.created_at AS created_at,n.attributes_json AS attributes_json";
                let sql = format!(
                    "MATCH (n:KgScalarFact),(a:KgSemanticEntity) WHERE a.id=n.subject AND n.space_id=$space AND n.id>$after AND ($history OR n.state='active') AND ($predicate='' OR n.predicate=$predicate) AND ($context='' OR n.context=$context) AND (($type='' AND ($kind='' OR a.kind=$kind) AND ($name='' OR a.id IN $named)) OR ($type='fact' AND n.id=$anchor) OR ($type='entity' AND $outgoing AND a.id=$anchor) OR ($type='source' AND a.id IN $mentioned)) RETURN {fields} ORDER BY n.id LIMIT 256"
                );
                let result=tx.query_with(&sql,::graphdb::params!{"space"=>space,"after"=>&after,"history"=>history,"predicate"=>predicate,"context"=>context,"type"=>anchor_type,"kind"=>kind,"name"=>name,"named"=>&named,"anchor"=>&key,"outgoing"=>outgoing,"mentioned"=>&strings(&mentioned)},options)?;
                let mut cursor = result.rows();
                let mut page = Vec::new();
                while let Some(row) = cursor.next()? {
                    page.push(ScalarFactRow::decode_row(&row)?);
                }
                charge(
                    ctx,
                    BudgetUsage {
                        edges: page.len() as u64,
                        ..Default::default()
                    },
                )?;
                let count = page.len();
                if let Some(last) = page.last() {
                    after.clone_from(&last.id);
                }
                retain_bytes!(&page);
                out.scalar_facts.extend(page);
                if count < 256 {
                    break;
                }
            }
        }
        if matches!(anchor_type, "" | "episode") && predicate.is_empty() && name.is_empty() {
            node_pages!(
                EpisodeRow,
                episodes,
                "WHERE n.space_id=$space AND n.id>$after AND ($anchor='' OR n.id=$anchor) AND ($kind='' OR n.kind=$kind) AND ($context='' OR n.context=$context)",
                ::graphdb::params! {"space"=>space,"after"=>&after,"anchor"=>&key,"kind"=>kind,"context"=>context},
                nodes
            );
        }
        let mut entity_keys = out
            .semantic_relations
            .iter()
            .flat_map(|r| [r.source.clone(), r.target.clone()])
            .chain(out.scalar_facts.iter().map(|r| r.subject.clone()))
            .collect::<BTreeSet<_>>();
        entity_keys.extend(out.semantic_entities.iter().map(|e| e.id.clone()));
        for batch in entity_keys.into_iter().collect::<Vec<_>>().chunks(256) {
            node_pages!(
                SemanticEntityRow,
                semantic_entities,
                "WHERE n.id IN $ids AND n.id>$after",
                ::graphdb::params! {"ids"=>&strings(batch),"after"=>&after},
                nodes
            );
        }
        out.semantic_entities.sort_by(|a, b| a.id.cmp(&b.id));
        out.semantic_entities.dedup_by(|a, b| a.id == b.id);
        let entity_ids = out
            .semantic_entities
            .iter()
            .map(|e| e.entity_id.clone())
            .collect::<Vec<_>>();
        let fact_ids = out
            .semantic_relations
            .iter()
            .map(|r| r.fact_id.clone())
            .chain(out.scalar_facts.iter().map(|r| r.fact_id.clone()))
            .collect::<Vec<_>>();
        for (target_type, ids) in [("semantic_entity", entity_ids), ("fact", fact_ids.clone())] {
            for batch in ids.chunks(256) {
                node_pages!(
                    AssociationRow,
                    associations,
                    "WHERE n.target_space_id=$space AND n.target_type=$type AND n.target_id IN $ids AND n.id>$after",
                    ::graphdb::params! {"space"=>space,"type"=>target_type,"ids"=>&strings(batch),"after"=>&after},
                    candidates
                );
            }
        }
        if history {
            let ids = fact_ids
                .iter()
                .cloned()
                .chain(out.episodes.iter().map(|e| e.episode_id.clone()))
                .collect::<Vec<_>>();
            for batch in ids.chunks(256) {
                node_pages!(
                    SemanticChangeRow,
                    changes,
                    "WHERE n.target_space_id=$space AND n.target_id IN $ids AND n.id>$after",
                    ::graphdb::params! {"space"=>space,"ids"=>&strings(batch),"after"=>&after},
                    candidates
                );
            }
        }
        let episode_keys = out
            .associations
            .iter()
            .filter_map(|a| match (&a.episode_space_id, &a.episode_id) {
                (Some(space_id), Some(id)) => Some(
                    Identity {
                        space_id: space_id.clone(),
                        id: id.clone(),
                    }
                    .storage_key(),
                ),
                _ => None,
            })
            .collect::<crate::Result<Vec<_>>>()?;
        for batch in episode_keys.chunks(256) {
            node_pages!(
                EpisodeRow,
                episodes,
                "WHERE n.id IN $ids AND n.id>$after",
                ::graphdb::params! {"ids"=>&strings(batch),"after"=>&after},
                nodes
            );
        }
        out.episodes.sort_by(|a, b| a.id.cmp(&b.id));
        out.episodes.dedup_by(|a, b| a.id == b.id);
        let source_ids = out
            .associations
            .iter()
            .map(|a| a.source_id.clone())
            .chain(out.episodes.iter().map(|e| e.source_id.clone()))
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        for batch in source_ids.chunks(256) {
            node_pages!(
                SemanticSourceRow,
                semantic_sources,
                "WHERE n.source_id IN $ids AND n.source_id>$after",
                ::graphdb::params! {"ids"=>&strings(batch),"after"=>&after},
                candidates
            );
        }
        // Mentions are fetched only for the selected entity endpoints.
        for batch in out
            .semantic_entities
            .iter()
            .filter(|_| !endpoint_only && !matches!(anchor_type, "fact" | "episode"))
            .map(|e| e.id.clone())
            .collect::<Vec<_>>()
            .chunks(256)
        {
            let result=tx.query_with("MATCH (a:KgEntity)-[e:KgMentions]->(b:KgSemanticEntity) WHERE b.id IN $ids AND ($anchor='' OR a.id=$anchor) RETURN a.id,b.id,e.data",::graphdb::params!{"ids"=>&strings(batch),"anchor"=>if anchor_type=="source" {key.as_str()}else{""}},options)?;
            let mut cursor = result.rows();
            while let Some(row) = cursor.next()? {
                charge(
                    ctx,
                    BudgetUsage {
                        edges: 1,
                        ..Default::default()
                    },
                )?;
                let data: String = row.try_get(2)?;
                retain_bytes!(&data);
                out.edge(
                    "KgMentions",
                    &row.try_get::<String>(0)?,
                    &row.try_get::<String>(1)?,
                    Some(data),
                );
            }
        }
        out.sort();
        out.decode_semantic()
    }
}
