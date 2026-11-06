//! Bounded current-object selection. Authorization precedes support aggregation.
use super::{
    BudgetUsage, Completion, CoverageDiagnostic, Direction, EvidenceProvider, EvidenceRead,
    ExploreRequest, ExploreResult, GraphReadNeeds, GraphReader, KgReadView, ObjectRef, QueryDomain,
    QueryError, QueryErrorKind, QueryExecution, QueryResult, RequestContext, SemanticAccess,
    SemanticObject, SemanticPartition, SemanticProvenance, SemanticQuery, SemanticReadResult,
    SemanticValue, SourceFilter,
};
use crate::semantic::{
    FactChange, FactObject, Identity, ObjectId, SemanticCatalog, SemanticChange,
    SemanticSourceState, SourceAssociation, SourceEvidence, SourceRole,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    time::Instant,
};

fn permits(filter: &SourceFilter, source: &str) -> bool {
    filter
        .sources
        .as_ref()
        .is_none_or(|sources| sources.iter().any(|s| s == source))
}
fn member(values: Option<&Vec<String>>, value: &str) -> bool {
    values.is_none_or(|values| values.iter().any(|v| v == value))
}
fn read<R: GraphReader>(
    reader: &R,
    query: &SemanticQuery,
    context: &RequestContext,
) -> QueryResult<SemanticCatalog> {
    if let Some(selected) = reader.semantic_selection(query, context)? {
        return Ok(selected);
    }
    let space = &query.space_id;
    let mut catalog = SemanticCatalog::default();
    for partition in [
        SemanticPartition::Sources,
        SemanticPartition::Entities,
        SemanticPartition::Episodes,
        SemanticPartition::ScalarFacts,
        SemanticPartition::Relations,
        SemanticPartition::Associations,
        SemanticPartition::Changes,
        SemanticPartition::Mentions,
    ] {
        let mut offset = 0;
        loop {
            context.check()?;
            let page = reader.semantic_page(partition, space, offset, context)?;
            let nodes =
                page.sources.len() + page.entities.len() + page.episodes.len() + page.facts.len();
            let edges = page.associations.len() + page.changes.len() + page.mentions.len();
            context.charge(BudgetUsage {
                nodes: nodes as u64,
                edges: edges as u64,
                ..Default::default()
            })?;
            catalog.sources.extend(page.sources);
            catalog.entities.extend(page.entities);
            catalog.episodes.extend(page.episodes);
            catalog.facts.extend(page.facts);
            catalog.associations.extend(page.associations);
            catalog.changes.extend(page.changes);
            catalog.mentions.extend(page.mentions);
            if nodes + edges < 256 {
                break;
            }
            offset += 256;
        }
    }
    Ok(catalog)
}
struct Selection<'a> {
    data: &'a SemanticCatalog,
    query: &'a SemanticQuery,
    associations: BTreeMap<&'a ObjectId, Vec<&'a SourceAssociation>>,
    current: BTreeSet<&'a Identity>,
    build: &'a str,
    stamp: &'a super::ViewStamp,
}
impl Selection<'_> {
    fn object(&self, id: ObjectId, record: SemanticValue) -> SemanticObject {
        let provenance = self
            .associations
            .get(&id)
            .into_iter()
            .flatten()
            .filter(|a| self.query.history || a.role != SourceRole::Withdrawn)
            .map(|a| SemanticProvenance {
                evidence: a.evidence.clone(),
                role: a.role,
                read: None,
                unavailable_reason: a
                    .evidence
                    .locator
                    .is_none()
                    .then(|| "no_source_locator".into()),
            })
            .collect();
        SemanticObject {
            reference: ObjectRef {
                graph_build_id: self.build.into(),
                generation: self.stamp.generation.clone(),
                workspace_id: self.stamp.workspace_id.clone(),
                object: id,
            },
            record,
            provenance,
        }
    }
    fn entity_matches(&self, entity: &crate::semantic::SemanticEntity) -> bool {
        self.query
            .kind
            .as_ref()
            .is_none_or(|kind| kind == &entity.kind)
            && self
                .query
                .name
                .as_ref()
                .is_none_or(|name| name == &entity.name || entity.aliases.contains(name))
    }
    fn facts(
        &self,
        context: &RequestContext,
    ) -> QueryResult<(Vec<SemanticObject>, BTreeSet<Identity>)> {
        let named: BTreeSet<_> = self
            .data
            .entities
            .iter()
            .filter(|e| {
                self.entity_matches(e)
                    && self.associations.contains_key(&ObjectId::SemanticEntity {
                        identity: e.identity.clone(),
                    })
            })
            .map(|e| &e.identity)
            .collect();
        let mut objects = Vec::new();
        let mut identities = BTreeSet::new();
        for fact in &self.data.facts {
            context.check()?;
            let id = ObjectId::Fact {
                identity: fact.identity.clone(),
            };
            if !self.associations.contains_key(&id)
                || (!self.query.history && !self.current.contains(&fact.identity))
            {
                continue;
            }
            let object = match &fact.object {
                FactObject::Entity { identity } => Some(identity),
                FactObject::Scalar { .. } => None,
            };
            let selected = match self.query.anchor.as_ref().map(|a| &a.object) {
                Some(ObjectId::Fact { identity }) => identity == &fact.identity,
                Some(ObjectId::SemanticEntity { identity }) => {
                    (self.query.direction != Direction::Incoming && identity == &fact.subject)
                        || (self.query.direction != Direction::Outgoing && object == Some(identity))
                }
                Some(_) => false,
                None => {
                    (self.query.name.is_none() && self.query.kind.is_none())
                        || named.contains(&fact.subject)
                        || object.is_some_and(|id| named.contains(id))
                }
            };
            if !selected {
                continue;
            }
            identities.insert(fact.subject.clone());
            if let Some(object) = object {
                identities.insert(object.clone());
            }
            objects.push(self.object(id, SemanticValue::Fact(fact.clone())));
        }
        Ok((objects, identities))
    }
    fn entities(&self, identities: &BTreeSet<Identity>, contextual: bool) -> Vec<SemanticObject> {
        self.data
            .entities
            .iter()
            .filter_map(|entity| {
                let id = ObjectId::SemanticEntity {
                    identity: entity.identity.clone(),
                };
                let selected = identities.contains(&entity.identity)
                    || (!contextual
                        && self.entity_matches(entity)
                        && self.query.anchor.as_ref().is_none_or(|a| a.object == id));
                let provenance = self.associations.get(&id)?;
                let live = provenance.iter().any(|a| self.data.effective_support(a));
                (selected && (self.query.history || live))
                    .then(|| self.object(id, SemanticValue::Entity(entity.clone())))
            })
            .collect()
    }
    fn episodes(&self) -> Vec<SemanticObject> {
        let sources: BTreeMap<_, _> = self
            .data
            .sources
            .iter()
            .map(|s| (&s.source.source_id, s))
            .collect();
        self.data
            .episodes
            .iter()
            .filter_map(|episode| {
                let id = ObjectId::Episode {
                    identity: episode.identity.clone(),
                };
                if self.query.relation_type.is_some()
                    || self.query.name.is_some()
                    || self.query.anchor.as_ref().is_some_and(|a| a.object != id)
                    || self.query.kind.as_ref().is_some_and(|k| k != &episode.kind)
                {
                    return None;
                }
                let source = sources.get(&episode.source.source_id)?;
                if !self.query.history
                    && (episode.state != crate::semantic::EpisodeState::Active
                        || source.state == SemanticSourceState::Deleted)
                {
                    return None;
                }
                Some(SemanticObject {
                    reference: ObjectRef {
                        graph_build_id: self.build.into(),
                        generation: self.stamp.generation.clone(),
                        workspace_id: self.stamp.workspace_id.clone(),
                        object: id,
                    },
                    record: SemanticValue::Episode(episode.clone()),
                    provenance: vec![SemanticProvenance {
                        evidence: SourceEvidence {
                            episode: None,
                            source: episode.source.clone(),
                            locator: None,
                        },
                        role: SourceRole::Support,
                        read: None,
                        unavailable_reason: Some(
                            "body_reference_requires_host_versioned_content".into(),
                        ),
                    }],
                })
            })
            .collect()
    }
}
pub(in crate::query) fn select(
    data: &SemanticCatalog,
    query: &SemanticQuery,
    access: &SemanticAccess,
    build: &str,
    stamp: &super::ViewStamp,
    ctx: &RequestContext,
) -> QueryResult<SemanticReadResult> {
    let mut associations: BTreeMap<_, Vec<_>> = BTreeMap::new();
    for association in &data.associations {
        associations
            .entry(&association.target)
            .or_default()
            .push(association);
    }
    let selection = Selection {
        data,
        query,
        associations,
        current: data.current_facts().map(|f| &f.identity).collect(),
        build,
        stamp,
    };
    let (mut objects, mut identities) = selection.facts(ctx)?;
    let contextual =
        query.context.is_some() || access.contexts.is_some() || query.relation_type.is_some();
    if let Some(ObjectRef {
        object: ObjectId::SourceNode { node_id },
        ..
    }) = &query.anchor
    {
        for mention in &data.mentions {
            if &mention.owner_id == node_id && (!contextual || data.facts.iter().any(|fact| {
                (query.history || selection.current.contains(&fact.identity))
                    && selection.associations.contains_key(&ObjectId::Fact { identity: fact.identity.clone() })
                    && (fact.subject == mention.target || matches!(&fact.object, FactObject::Entity { identity } if identity == &mention.target))
            })) { identities.insert(mention.target.clone()); }
        }
    }
    objects.extend(selection.entities(&identities, contextual));
    objects.extend(selection.episodes());
    let visible: BTreeSet<_> = objects.iter().map(|o| &o.reference.object).collect();
    let changes = if query.history {
        data.changes
            .iter()
            .filter(|change| {
                let id = match change {
                    SemanticChange::Fact { change } => ObjectId::Fact {
                        identity: change.target().clone(),
                    },
                    SemanticChange::Episode { change } => ObjectId::Episode {
                        identity: change.target.clone(),
                    },
                };
                visible.contains(&id)
            })
            .cloned()
            .collect()
    } else {
        vec![]
    };
    let mentions = data.mentions.iter().filter(|mention| {
        visible.contains(&ObjectId::SemanticEntity { identity: mention.target.clone() })
            && query.anchor.as_ref().is_none_or(|anchor| !matches!(&anchor.object, ObjectId::SourceNode { node_id } if node_id != &mention.owner_id))
    }).cloned().collect();
    Ok(SemanticReadResult {
        sources: vec![],
        objects,
        changes,
        mentions,
        evidence: vec![],
    })
}
fn change_evidence(change: &mut SemanticChange) -> &mut SourceEvidence {
    match change {
        SemanticChange::Fact {
            change: FactChange::Supersede { evidence, .. } | FactChange::Retract { evidence, .. },
        } => evidence,
        SemanticChange::Episode { change } => &mut change.evidence,
    }
}
pub(in crate::query) fn evidence<L, E: EvidenceProvider<L>>(
    provider: &E,
    view: &KgReadView<L>,
    result: &mut SemanticReadResult,
    context: &RequestContext,
) -> QueryResult<()> {
    let refs = result
        .objects
        .iter()
        .flat_map(|o| o.provenance.iter().map(|p| p.evidence.clone()))
        .chain(result.changes.iter().map(|c| c.evidence().clone()))
        .collect::<Vec<_>>();
    let reads = provider.read_semantic_evidence(view, &refs, context)?;
    if reads.len() != refs.len() {
        return Err(QueryError::invalid("semantic evidence count mismatch"));
    }
    let mut reads = reads.into_iter();
    for provenance in result.objects.iter_mut().flat_map(|o| &mut o.provenance) {
        match reads.next().expect("response count checked") {
            Some(EvidenceRead::Unavailable { reason, .. }) => {
                provenance.evidence.locator = None;
                provenance.unavailable_reason = Some(reason);
            }
            read => provenance.read = read,
        }
    }
    for change in &mut result.changes {
        if !matches!(
            reads.next().expect("response count checked"),
            Some(EvidenceRead::Verified { .. })
        ) {
            change_evidence(change).locator = None;
        }
    }
    let refs = result
        .mentions
        .iter()
        .map(|m| m.evidence.clone())
        .collect::<Vec<_>>();
    result.evidence = provider.read_evidence(view, &refs, context)?;
    if result.evidence.len() != refs.len()
        || result
            .evidence
            .iter()
            .any(|e| matches!(e, EvidenceRead::Unavailable { .. }))
    {
        return Err(QueryError::new(
            QueryErrorKind::ViewMismatch,
            "semantic_read",
            "mention source evidence unavailable",
        ));
    }
    Ok(())
}
fn source_nodes<R: GraphReader>(
    reader: &R,
    result: &mut SemanticReadResult,
    build: &str,
    stamp: &super::ViewStamp,
    ctx: &RequestContext,
) -> QueryResult<()> {
    let ids = result
        .mentions
        .iter()
        .map(|m| m.owner_id.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    for batch in ids.chunks(256) {
        let nodes = reader.source_nodes(batch, ctx)?;
        ctx.charge(BudgetUsage {
            nodes: nodes.len() as u64,
            ..Default::default()
        })?;
        if nodes.len() != batch.len() {
            return Err(QueryError::new(
                QueryErrorKind::ViewMismatch,
                "semantic_read",
                "mention owner missing",
            ));
        }
        result
            .objects
            .extend(nodes.into_iter().map(|node| SemanticObject {
                reference: ObjectRef {
                    graph_build_id: build.into(),
                    generation: stamp.generation.clone(),
                    workspace_id: stamp.workspace_id.clone(),
                    object: ObjectId::SourceNode {
                        node_id: node.id.clone(),
                    },
                },
                record: SemanticValue::SourceNode(node),
                provenance: vec![],
            }));
    }
    Ok(())
}
pub(super) fn explore<L, R: GraphReader, E: EvidenceProvider<L>>(
    reader: &R,
    provider: &E,
    view: &KgReadView<L>,
    request: &ExploreRequest,
    query: &SemanticQuery,
    context: &RequestContext,
) -> QueryResult<ExploreResult> {
    context.check()?;
    let started = Instant::now();
    view.require(&GraphReadNeeds {
        domain: QueryDomain::Semantic,
        relations: vec![],
        text_mapping: false,
    })?;
    let access = view.semantic_access()?;
    let mut result = SemanticReadResult::default();
    let empty_sources = [&access.sources, &request.boundary, &request.result_filter]
        .iter()
        .any(|f| f.sources.as_ref().is_some_and(Vec::is_empty));
    if member(access.spaces.as_ref(), &query.space_id) && !empty_sources {
        let mut data = read(reader, query, context)?;
        data.sources.retain(|s| {
            permits(&access.sources, &s.source.source_id)
                && permits(&request.boundary, &s.source.source_id)
                && permits(&request.result_filter, &s.source.source_id)
        });
        let sources = data
            .sources
            .iter()
            .map(|s| &s.source.source_id)
            .collect::<BTreeSet<_>>();
        data.associations
            .retain(|a| sources.contains(&a.evidence.source.source_id));
        data.changes
            .retain(|c| sources.contains(&c.evidence().source.source_id));
        data.mentions
            .retain(|m| sources.contains(&m.evidence.source_id));
        let allowed_context = |name: &str| {
            member(access.contexts.as_ref(), name)
                && query.context.as_ref().is_none_or(|c| c == name)
        };
        data.facts.retain(|f| {
            allowed_context(&f.context)
                && query
                    .relation_type
                    .as_ref()
                    .is_none_or(|p| p == &f.relation_type)
        });
        data.episodes.retain(|e| allowed_context(&e.context));
        let build = &view.stamp().require_graph()?.build_id;
        result = select(&data, query, access, build, view.stamp(), context)?;
        let used = result
            .objects
            .iter()
            .flat_map(|o| o.provenance.iter().map(|p| &p.evidence.source.source_id))
            .chain(
                result
                    .changes
                    .iter()
                    .map(|c| &c.evidence().source.source_id),
            )
            .chain(result.mentions.iter().map(|m| &m.evidence.source_id))
            .collect::<BTreeSet<_>>();
        result.sources = data
            .sources
            .iter()
            .filter(|source| used.contains(&source.source.source_id))
            .cloned()
            .collect();
        evidence(provider, view, &mut result, context)?;
        source_nodes(reader, &mut result, build, view.stamp(), context)?;
    }
    result
        .objects
        .sort_by(|a, b| a.reference.object.cmp(&b.reference.object));
    let mut output = ExploreResult {
        next_cursor: None,
        relation_types: Vec::new(),
        semantic: Some(result),
        view: view.stamp().clone(),
        locations: vec![],
        nodes: vec![],
        relations: vec![],
        paths: vec![],
        references: vec![],
        evidence: vec![],
        coverage: CoverageDiagnostic {
            domain: QueryDomain::Semantic,
            ready: true,
            note: "current objects; explicit business history only".into(),
        },
        execution: QueryExecution {
            status: Completion::Complete,
            stop_reason: None,
            usage: context.usage()?,
            qualification_micros: 0,
            query_micros: started.elapsed().as_micros().try_into().unwrap_or(u64::MAX),
        },
    };
    super::explore::account_output(&mut output, context, true)?;
    Ok(output)
}
