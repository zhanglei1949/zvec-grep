//! Search identities retained before presentation reduces a hit to one excerpt.
use std::collections::BTreeMap;

use crate::{
    EngineError, EngineResult,
    api::context::ContextResult,
    domain::{Content, EntityFragment, EntityId, FileRecord, Range},
    utils::sha256_hex,
};

use super::{context::NormalizedContextGroup, pipeline::SearchPlanResult};

#[derive(Clone, Debug, Eq, PartialEq)]
pub(in crate::pipelines) struct RetainedHit {
    pub file: FileRecord,
    pub entity_id: EntityId,
    pub entity_range: Range,
    pub content_digest: String,
    pub content_bytes: Option<u64>,
    pub fragment: EntityFragment,
    pub priority: usize,
    pub query_sources: Vec<String>,
}

#[cfg(feature = "knowledge-graph-graphdb")]
impl RetainedHit {
    pub(in crate::pipelines) fn verify(
        &self,
        storage: &crate::storage::IndexStore,
    ) -> EngineResult<()> {
        let entity = storage
            .graph_entities(&[self.entity_id.as_str().into()])?
            .pop()
            .ok_or_else(|| EngineError::storage_failure("retrieved entity missing"))?;
        if entity.id != self.entity_id
            || entity.file_id != self.file.id
            || entity.source_range != self.entity_range
            || !entity.fragments.contains(&self.fragment)
            || sha256_hex(&serde_json::to_vec(&entity.content).map_err(|_| inconsistent())?)
                != self.content_digest
        {
            return Err(EngineError::storage_failure(
                "retrieved entity binding changed",
            ));
        }
        Ok(())
    }
}

pub(super) struct RankedContext {
    result: ContextResult,
    hits: Vec<RetainedHit>,
}

impl RankedContext {
    pub fn new(result: ContextResult, mut hits: Vec<RetainedHit>) -> Self {
        let priorities = result
            .items
            .iter()
            .filter_map(|item| item.entity_id.as_deref().map(|id| (id, item.rank)))
            .collect::<BTreeMap<_, _>>();
        hits.retain_mut(|hit| {
            let Some(priority) = priorities.get(hit.entity_id.as_str()) else {
                return false;
            };
            hit.priority = *priority;
            true
        });
        hits.sort_by(|a, b| {
            a.priority
                .cmp(&b.priority)
                .then(a.entity_id.cmp(&b.entity_id))
                .then(a.fragment.id.cmp(&b.fragment.id))
        });
        Self { result, hits }
    }

    pub fn into_parts(self) -> (ContextResult, Vec<RetainedHit>) {
        (self.result, self.hits)
    }
}

pub(super) fn collect(
    searches: &[SearchPlanResult],
    groups: &[NormalizedContextGroup],
    fused_sources: Option<&[NormalizedContextGroup]>,
) -> EngineResult<Vec<RetainedHit>> {
    let mut identities = BTreeMap::new();
    let mut retained = BTreeMap::<_, RetainedHit>::new();
    for (search, group) in searches.iter().zip(groups) {
        let sources = fused_sources.unwrap_or_else(|| std::slice::from_ref(group));
        let origins = sources
            .iter()
            .flat_map(|source| source.routes.iter().map(|_| source.id.as_str()));
        let route_sources = search
            .routes
            .iter()
            .zip(origins)
            .map(|(route, source)| (route.id.as_str(), source))
            .collect::<BTreeMap<_, _>>();
        for hit in &search.hits {
            let entity = &hit.entity;
            if entity.file_id != hit.file.id
                || EntityId::new(entity.file_id, &entity.content, entity.source_range)? != entity.id
            {
                return Err(inconsistent());
            }
            entity.validate()?;
            let identity = (
                hit.file.clone(),
                entity.source_range,
                sha256_hex(&serde_json::to_vec(&entity.content).map_err(|_| inconsistent())?),
                entity.fragments.clone(),
            );
            if identities
                .insert(entity.id.clone(), identity.clone())
                .is_some_and(|previous| previous != identity)
            {
                return Err(inconsistent());
            }
            for evidence in &hit.evidence {
                if !entity.fragments.contains(&evidence.fragment) {
                    return Err(inconsistent());
                }
                let route = evidence
                    .route_id
                    .split('@')
                    .next()
                    .ok_or_else(inconsistent)?
                    .trim_end_matches(".prefer-symbol");
                let origin = route_sources.get(route).ok_or_else(inconsistent)?;
                let candidate = retained
                    .entry((entity.id.clone(), evidence.fragment.id.clone()))
                    .or_insert_with(|| RetainedHit {
                        file: hit.file.clone(),
                        entity_id: entity.id.clone(),
                        entity_range: entity.source_range,
                        content_digest: identity.2.clone(),
                        content_bytes: match &entity.content {
                            Content::Text(text) => Some(text.len() as u64),
                            _ => None,
                        },
                        fragment: evidence.fragment.clone(),
                        priority: 0,
                        query_sources: Vec::new(),
                    });
                if !candidate.query_sources.iter().any(|query| query == origin) {
                    candidate.query_sources.push((*origin).to_owned());
                }
            }
        }
    }
    Ok(retained.into_values().collect())
}

fn inconsistent() -> EngineError {
    EngineError::storage_failure("inconsistent retrieved entity or fragment identity")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        api::context::{ContextOptions, result::MatchedBy},
        domain::{
            ByteRange, Entity, FileId, FileIndexStatus, FileSnapshot, FragmentId, SourcePath,
            TextRange,
        },
        pipelines::indexed_search::{
            context::normalize_context_request,
            pipeline::{SearchEvidence, SearchHit},
        },
    };

    fn search() -> SearchPlanResult {
        let content = Content::Text("a中😀\r\nb".into());
        let file_id = FileId::new(7);
        let source_range = Range::Text(
            TextRange::from_coordinates(9, 20, 2, 3, 1, 1).expect("valid deterministic fixture"),
        );
        let id =
            EntityId::new(file_id, &content, source_range).expect("valid deterministic fixture");
        let fragments = vec![
            EntityFragment {
                id: FragmentId::new(&id, 0),
                range: Range::Byte(ByteRange::new(1, 8).expect("valid deterministic fixture")),
            },
            EntityFragment {
                id: FragmentId::new(&id, 1),
                range: Range::Full,
            },
        ];
        SearchPlanResult {
            routes: vec![super::super::pipeline::ResolvedSearchRoute {
                id: "fts".into(),
                mode: crate::api::context::options::ContextRouteMode::Fts,
                query: "fixture".into(),
            }],
            timings: vec![],
            hits: vec![SearchHit {
                entity: Entity {
                    id,
                    file_id,
                    source_range,
                    content,
                    metadata: None,
                    fragments: fragments.clone(),
                },
                file: FileRecord {
                    id: file_id,
                    relative_path: SourcePath::new("src/a.rs")
                        .expect("valid deterministic fixture"),
                    snapshot: FileSnapshot {
                        size_bytes: 20,
                        modified_epoch_ms: None,
                        content_hash: Some("a".repeat(64)),
                    },
                    index_status: FileIndexStatus::NotIndexed,
                },
                evidence: fragments
                    .into_iter()
                    .map(|fragment| SearchEvidence {
                        fragment,
                        route_id: "fts".into(),
                    })
                    .collect(),
                rank: 1,
                score: 1.0,
                matched_by: MatchedBy::Fts,
                trace: None,
            }],
        }
    }

    #[test]
    fn real_fragments_keep_their_own_query_provenance_without_trace() {
        let request = normalize_context_request(&ContextOptions {
            queries: vec!["one".into(), "two".into()],
            ..Default::default()
        })
        .expect("valid deterministic fixture");
        let first = search();
        let mut second = first.clone();
        second.hits[0].evidence.remove(1);
        let hits = collect(&[first.clone(), second], &request.groups, None)
            .expect("valid deterministic fixture");
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].query_sources, ["Q1", "Q2"]);
        assert_eq!(hits[1].query_sources, ["Q1"]);
        assert_eq!(hits[0].fragment, first.hits[0].evidence[0].fragment);
        assert_eq!(hits[0].file, first.hits[0].file);
        assert_eq!(hits[0].entity_id, first.hits[0].entity.id);
        assert_eq!(hits[0].entity_range, first.hits[0].entity.source_range);
        assert_eq!(hits[0].content_bytes, Some(11));
        assert_eq!(hits[0].content_digest.len(), 64);
    }

    #[test]
    fn inconsistent_entity_fragment_or_group_snapshot_fails_closed() {
        let request = normalize_context_request(&ContextOptions {
            queries: vec!["one".into(), "two".into()],
            ..Default::default()
        })
        .expect("valid deterministic fixture");
        let first = search();
        for variant in 0..3 {
            let mut changed = first.clone();
            match variant {
                0 => changed.hits[0].entity.content = Content::Text("different".into()),
                1 => changed.hits[0].evidence[0].fragment.range = Range::Full,
                _ => changed.hits[0].file.snapshot.content_hash = Some("b".repeat(64)),
            }
            assert!(collect(&[first.clone(), changed], &request.groups, None).is_err());
        }
    }

    #[test]
    fn fused_queries_keep_each_fragments_original_group_even_without_trace() {
        use crate::api::context::options::ContextRouteMode;
        use crate::pipelines::indexed_search::pipeline::ResolvedSearchRoute;
        let request = normalize_context_request(&ContextOptions {
            queries: vec!["one".into(), "two".into()],
            ..Default::default()
        })
        .expect("request");
        let mut search = search();
        search.routes = ["fts", "vector", "fts-2", "vector-2"]
            .into_iter()
            .map(|id| ResolvedSearchRoute {
                id: id.into(),
                mode: if id.starts_with("fts") {
                    ContextRouteMode::Fts
                } else {
                    ContextRouteMode::Vector
                },
                query: "fixture".into(),
            })
            .collect();
        search.hits[0].evidence[1].route_id = "fts-2.prefer-symbol".into();
        let mut duplicate = search.hits[0].evidence[0].clone();
        duplicate.route_id = "vector-2".into();
        search.hits[0].evidence.push(duplicate);
        let hits = collect(&[search], &request.groups[..1], Some(&request.groups))
            .expect("fused metadata");
        assert_eq!(hits[0].query_sources, ["Q1", "Q2"]);
        assert_eq!(hits[1].query_sources, ["Q2"]);
    }
}
