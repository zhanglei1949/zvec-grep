mod selection;
use super::{FactRows, GraphResult, invalid, json, tag, unjson, untag};
use crate::{
    graph::schema::{
        AssociationRow, DeferredMentionRow, EpisodeLinkRow, EpisodeRow, NativeTime, ScalarFactRow,
        SemanticChangeRow, SemanticEntityRow, SemanticOperationRow, SemanticRelationRow,
        SemanticSourceRow,
    },
    processing::{canonical_json, stable_digest},
    semantic::{
        Episode, Fact, FactObject, Identity, Mention, ObjectId, SemanticCatalog, SemanticEntity,
        SemanticSource, SourceAssociation, SourceEvidence, SourceRef,
    },
};

fn identity(key: &str) -> GraphResult<Identity> {
    let (space_id, id) = serde_json::from_str(key).map_err(|_| invalid())?;
    Ok(Identity { space_id, id })
}
fn parse<T: serde::de::DeserializeOwned>(value: &str) -> GraphResult<T> {
    serde_json::from_str(value).map_err(|_| invalid())
}

impl FactRows {
    pub(in crate::graph::graphdb) fn semantic_page(
        tx: &mut ::graphdb::Transaction<'_>,
        options: &::graphdb::QueryOptions,
        partition: crate::query::SemanticPartition,
        space: &str,
        offset: u64,
    ) -> GraphResult<SemanticCatalog> {
        use crate::query::SemanticPartition;
        let mut rows = Self::default();
        match partition {
            SemanticPartition::Sources => {
                rows.semantic_sources =
                    SemanticSourceRow::select(tx, options, "", ().into(), offset)?;
            }
            SemanticPartition::Entities => {
                rows.semantic_entities = SemanticEntityRow::select(
                    tx,
                    options,
                    "WHERE n.space_id=$space",
                    ::graphdb::params! {"space"=>space},
                    offset,
                )?;
            }
            SemanticPartition::Episodes => {
                rows.episodes = EpisodeRow::select(
                    tx,
                    options,
                    "WHERE n.space_id=$space",
                    ::graphdb::params! {"space"=>space},
                    offset,
                )?;
            }
            SemanticPartition::ScalarFacts => {
                rows.scalar_facts = ScalarFactRow::select(
                    tx,
                    options,
                    "WHERE n.space_id=$space",
                    ::graphdb::params! {"space"=>space},
                    offset,
                )?;
            }
            SemanticPartition::Associations => {
                rows.associations = AssociationRow::select(
                    tx,
                    options,
                    "WHERE n.target_space_id=$space",
                    ::graphdb::params! {"space"=>space},
                    offset,
                )?;
            }
            SemanticPartition::Changes => {
                rows.changes = SemanticChangeRow::select(
                    tx,
                    options,
                    "WHERE n.target_space_id=$space",
                    ::graphdb::params! {"space"=>space},
                    offset,
                )?;
            }
            SemanticPartition::Relations => {
                let result = tx.query_with(&format!("MATCH (a:KgSemanticEntity)-[e:KgSemanticRelation]->(b:KgSemanticEntity) WHERE e.space_id=$space RETURN a.id AS source,b.id AS target,e.id AS id,e.fact_id AS fact_id,e.space_id AS space_id,e.predicate AS predicate,e.context AS context,e.state AS state,e.supersedes AS supersedes,e.changed_at AS changed_at,e.created_at AS created_at,e.attributes_json AS attributes_json ORDER BY e.id SKIP {offset} LIMIT 256"), ::graphdb::params!{"space"=>space}, options)?;
                let mut cursor = result.rows();
                while let Some(row) = cursor.next()? {
                    rows.semantic_relations
                        .push(SemanticRelationRow::decode_row(&row)?);
                }
            }
            SemanticPartition::Mentions => {
                let result = tx.query_with(&format!("MATCH (a:KgEntity)-[e:KgMentions]->(b:KgSemanticEntity) WHERE b.space_id=$space RETURN a.id,b.id,e.data ORDER BY a.id,b.id,e.data SKIP {offset} LIMIT 256"), ::graphdb::params!{"space"=>space}, options)?;
                let mut cursor = result.rows();
                while let Some(row) = cursor.next()? {
                    rows.edge(
                        "KgMentions",
                        &row.try_get::<String>(0)?,
                        &row.try_get::<String>(1)?,
                        Some(row.try_get(2)?),
                    );
                }
            }
        }
        rows.decode_semantic()
    }
    #[expect(
        clippy::too_many_lines,
        reason = "fixed row mapping for one schema version"
    )]
    pub(super) fn encode_semantic(&mut self, catalog: &SemanticCatalog) -> GraphResult<()> {
        for source in &catalog.sources {
            self.semantic_sources.push(SemanticSourceRow {
                source_id: source.source.source_id.clone(),
                source_version: source.source.source_version.clone(),
                state: tag(&source.state)?,
                semantic_version: source.semantic_version.clone(),
            });
        }
        for entity in &catalog.entities {
            self.semantic_entities.push(SemanticEntityRow {
                id: entity.identity.storage_key()?,
                entity_id: entity.identity.id.clone(),
                space_id: entity.identity.space_id.clone(),
                kind: entity.kind.clone(),
                name: entity.name.clone(),
                aliases_json: canonical_json(&entity.aliases)?,
                attributes_json: canonical_json(&entity.attributes)?,
            });
        }
        for link in &catalog.episode_links {
            self.episode_links.push(EpisodeLinkRow {
                source: link.source.storage_key()?,
                target: link.target.storage_key()?,
                id: link.identity.storage_key()?,
                link_id: link.identity.id.clone(),
                space_id: link.identity.space_id.clone(),
                relation_type: tag(&link.relation_type)?,
            });
        }
        for episode in &catalog.episodes {
            self.episodes.push(EpisodeRow {
                id: episode.identity.storage_key()?,
                episode_id: episode.identity.id.clone(),
                space_id: episode.identity.space_id.clone(),
                kind: episode.kind.clone(),
                source_id: episode.source.source_id.clone(),
                source_version: episode.source.source_version.clone(),
                context: episode.context.clone(),
                occurred_at: episode
                    .occurred_at_ms
                    .map(NativeTime::from_millis)
                    .transpose()?,
                ingested_at: NativeTime::from_millis(episode.ingested_at_ms)?,
                content_ref: episode.content_ref.clone(),
                state: tag(&episode.state)?,
            });
        }
        self.encode_semantic_facts(&catalog.facts)?;
        for change in &catalog.changes {
            let target = change.target();
            self.changes.push(SemanticChangeRow {
                id: stable_digest(change)?,
                target_type: match change {
                    crate::semantic::SemanticChange::Fact { .. } => "fact",
                    crate::semantic::SemanticChange::Episode { .. } => "episode",
                }
                .into(),
                target_space_id: target.space_id.clone(),
                target_id: target.id.clone(),
                changed_at: NativeTime::from_millis(change.changed_at_ms())?,
                data: json(change)?,
            });
        }
        if let Some(operation) = &catalog.last_operation {
            self.operations.push(SemanticOperationRow {
                id: "latest".into(),
                data: json(operation)?,
            });
        }
        for association in &catalog.associations {
            let (target_type, target) = match &association.target {
                ObjectId::SemanticEntity { identity } => ("semantic_entity", identity),
                ObjectId::Fact { identity } => ("fact", identity),
                _ => return Err(invalid()),
            };
            self.associations.push(AssociationRow {
                id: stable_digest(association)?,
                target_type: target_type.into(),
                target_space_id: target.space_id.clone(),
                target_id: target.id.clone(),
                source_id: association.evidence.source.source_id.clone(),
                source_version: association.evidence.source.source_version.clone(),
                role: tag(&association.role)?,
                episode_space_id: association
                    .evidence
                    .episode
                    .as_ref()
                    .map(|id| id.space_id.clone()),
                episode_id: association
                    .evidence
                    .episode
                    .as_ref()
                    .map(|id| id.id.clone()),
                locator_json: association
                    .evidence
                    .locator
                    .as_ref()
                    .map(canonical_json)
                    .transpose()?,
            });
        }
        for mention in &catalog.deferred_mentions {
            self.deferred_mentions.push(DeferredMentionRow {
                id: mention.ref_id.clone(),
                data: json(mention)?,
            });
        }
        for mention in &catalog.mentions {
            self.edge(
                "KgMentions",
                &mention.owner_id,
                &mention.target.storage_key()?,
                Some(json(mention)?),
            );
        }
        Ok(())
    }

    pub(super) fn decode_semantic(&self) -> GraphResult<SemanticCatalog> {
        let mut catalog = SemanticCatalog::default();
        for row in &self.semantic_sources {
            catalog.sources.push(SemanticSource {
                source: SourceRef {
                    source_id: row.source_id.clone(),
                    source_version: row.source_version.clone(),
                },
                state: untag(&row.state)?,
                semantic_version: row.semantic_version.clone(),
            });
        }
        for row in &self.semantic_entities {
            catalog.entities.push(SemanticEntity {
                identity: Identity {
                    space_id: row.space_id.clone(),
                    id: row.entity_id.clone(),
                },
                kind: row.kind.clone(),
                name: row.name.clone(),
                aliases: parse(&row.aliases_json)?,
                attributes: parse(&row.attributes_json)?,
            });
        }
        for row in &self.episodes {
            catalog.episodes.push(Episode {
                identity: Identity {
                    space_id: row.space_id.clone(),
                    id: row.episode_id.clone(),
                },
                kind: row.kind.clone(),
                source: SourceRef {
                    source_id: row.source_id.clone(),
                    source_version: row.source_version.clone(),
                },
                context: row.context.clone(),
                occurred_at_ms: row.occurred_at.map(NativeTime::millis).transpose()?,
                ingested_at_ms: row.ingested_at.millis()?,
                content_ref: row.content_ref.clone(),
                state: untag(&row.state)?,
            });
        }
        for row in &self.episode_links {
            catalog.episode_links.push(crate::semantic::EpisodeLink {
                identity: Identity {
                    space_id: row.space_id.clone(),
                    id: row.link_id.clone(),
                },
                source: identity(&row.source)?,
                target: identity(&row.target)?,
                relation_type: untag(&row.relation_type)?,
            });
        }
        catalog.facts = self.decode_semantic_facts()?;
        for row in &self.associations {
            let target = Identity {
                space_id: row.target_space_id.clone(),
                id: row.target_id.clone(),
            };
            catalog.associations.push(SourceAssociation {
                target: match row.target_type.as_str() {
                    "semantic_entity" => ObjectId::SemanticEntity { identity: target },
                    "fact" => ObjectId::Fact { identity: target },
                    _ => return Err(invalid()),
                },
                evidence: SourceEvidence {
                    episode: match (&row.episode_space_id, &row.episode_id) {
                        (Some(space_id), Some(id)) => Some(Identity {
                            space_id: space_id.clone(),
                            id: id.clone(),
                        }),
                        (None, None) => None,
                        _ => return Err(invalid()),
                    },
                    source: SourceRef {
                        source_id: row.source_id.clone(),
                        source_version: row.source_version.clone(),
                    },
                    locator: row.locator_json.as_deref().map(parse).transpose()?,
                },
                role: untag(&row.role)?,
            });
        }
        for edge in self.edges.iter().filter(|edge| edge.name == "KgMentions") {
            let mention: Mention = unjson(edge.version.as_deref().ok_or_else(invalid)?)?;
            catalog.mentions.push(mention);
        }
        for row in &self.deferred_mentions {
            catalog.deferred_mentions.push(unjson(&row.data)?);
        }
        for row in &self.changes {
            catalog.changes.push(unjson(&row.data)?);
        }
        catalog.last_operation = match self.operations.as_slice() {
            [] => None,
            [row] if row.id == "latest" => Some(unjson(&row.data)?),
            _ => return Err(invalid()),
        };
        catalog.normalize()?;
        Ok(catalog)
    }
    fn encode_semantic_facts(&mut self, facts: &[Fact]) -> GraphResult<()> {
        for fact in facts {
            let previous = fact
                .supersedes
                .as_ref()
                .map(Identity::storage_key)
                .transpose()?;
            match &fact.object {
                FactObject::Entity { identity } => {
                    self.semantic_relations.push(SemanticRelationRow {
                        source: fact.subject.storage_key()?,
                        target: identity.storage_key()?,
                        id: fact.identity.storage_key()?,
                        fact_id: fact.identity.id.clone(),
                        space_id: fact.identity.space_id.clone(),
                        predicate: fact.relation_type.clone(),
                        context: fact.context.clone(),
                        state: tag(&fact.state)?,
                        supersedes: previous,
                        changed_at: NativeTime::from_millis(fact.changed_at_ms)?,
                        created_at: fact
                            .created_at_ms
                            .map(NativeTime::from_millis)
                            .transpose()?,
                        attributes_json: canonical_json(&fact.attributes)?,
                    });
                }
                FactObject::Scalar { value } => self.scalar_facts.push(ScalarFactRow {
                    id: fact.identity.storage_key()?,
                    fact_id: fact.identity.id.clone(),
                    space_id: fact.identity.space_id.clone(),
                    subject: fact.subject.storage_key()?,
                    predicate: fact.relation_type.clone(),
                    object_json: canonical_json(value)?,
                    context: fact.context.clone(),
                    state: tag(&fact.state)?,
                    supersedes: previous,
                    changed_at: NativeTime::from_millis(fact.changed_at_ms)?,
                    created_at: fact
                        .created_at_ms
                        .map(NativeTime::from_millis)
                        .transpose()?,
                    attributes_json: canonical_json(&fact.attributes)?,
                }),
            }
        }
        Ok(())
    }
    fn decode_semantic_facts(&self) -> GraphResult<Vec<Fact>> {
        let mut facts = Vec::new();
        for row in &self.semantic_relations {
            facts.push(Fact {
                created_at_ms: row.created_at.map(NativeTime::millis).transpose()?,
                identity: Identity {
                    space_id: row.space_id.clone(),
                    id: row.fact_id.clone(),
                },
                subject: identity(&row.source)?,
                relation_type: row.predicate.clone(),
                object: FactObject::Entity {
                    identity: identity(&row.target)?,
                },
                context: row.context.clone(),
                state: untag(&row.state)?,
                supersedes: row.supersedes.as_deref().map(identity).transpose()?,
                changed_at_ms: row.changed_at.millis()?,
                attributes: parse(&row.attributes_json)?,
            });
        }
        for row in &self.scalar_facts {
            facts.push(Fact {
                created_at_ms: row.created_at.map(NativeTime::millis).transpose()?,
                identity: Identity {
                    space_id: row.space_id.clone(),
                    id: row.fact_id.clone(),
                },
                subject: identity(&row.subject)?,
                relation_type: row.predicate.clone(),
                object: FactObject::Scalar {
                    value: parse(&row.object_json)?,
                },
                context: row.context.clone(),
                state: untag(&row.state)?,
                supersedes: row.supersedes.as_deref().map(identity).transpose()?,
                changed_at_ms: row.changed_at.millis()?,
                attributes: parse(&row.attributes_json)?,
            });
        }
        Ok(facts)
    }
}
