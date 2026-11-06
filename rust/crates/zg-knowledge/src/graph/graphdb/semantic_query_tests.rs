use super::{tests::context, *};
use crate::{processing::stable_digest, query::*, semantic::*};
use std::time::{Duration, Instant};

type TestResult = Result<(), Box<dyn std::error::Error>>;
fn input(name: &str) -> SemanticInput {
    let fixture: serde_json::Value = serde_json::from_str(include_str!(
        "../../../tests/fixtures/semantic-source-changes-v1.json"
    ))
    .expect("frozen inputs");
    serde_json::from_value(
        fixture["cases"]
            .as_array()
            .expect("cases")
            .iter()
            .find(|case| case["name"] == name)
            .expect("case")["input"]
            .clone(),
    )
    .expect("input")
}
fn base() -> FactCatalog {
    FactCatalog::resolve(
        vec![],
        stable_digest(&serde_json::json!({})).expect("policy"),
        stable_digest(&"selection").expect("selection"),
        &context(),
    )
    .expect("empty")
    .apply_sources(&input("initial"), &context())
    .expect("seed")
}
fn request(history: bool) -> ExploreRequest {
    ExploreRequest {
        page_size: 64,
        cursor: None,
        relation_filters: Vec::new(),
        domain: QueryDomain::Semantic,
        operation: ExploreOperation::Semantic {
            query: SemanticQuery {
                space_id: "workspace".into(),
                anchor: None,
                context: Some("project-a".into()),
                name: None,
                kind: None,
                relation_type: Some("owner".into()),
                history,
                direction: Direction::Both,
            },
        },
        boundary: SourceFilter::default(),
        result_filter: SourceFilter::default(),
    }
}
fn ctx() -> RequestContext {
    RequestContext::new(
        BudgetLimits::default(),
        tokio_util::sync::CancellationToken::new(),
        Instant::now() + Duration::from_secs(30),
    )
    .expect("query budget")
}
fn view(id: &str, access: SemanticAccess) -> KgReadView<()> {
    granted_view(id, access, false)
}
fn granted_view(id: &str, access: SemanticAccess, text_mapping: bool) -> KgReadView<()> {
    KgReadView::new(
        ViewStamp {
            workspace_id: "workspace".into(),
            generation: "generation".into(),
            text_revision: 1,
            graph: Some(GraphStamp {
                build_id: id.into(),
                schema: "v7".into(),
                rules: "m3".into(),
                configuration: "fixture".into(),
                catalog_revision: "current".into(),
            }),
        },
        (),
        &ReadViewRequest {
            graph: GraphRequirement::Required,
            needs: Some(GraphReadNeeds {
                domain: QueryDomain::Semantic,
                relations: vec![],
                text_mapping: false,
            }),
        },
        vec![GraphReadCapabilities {
            domain: QueryDomain::Semantic,
            relations: vec![
                RelationKind::Mentions,
                RelationKind::SemanticRelation,
                RelationKind::EpisodeLink,
            ],
            text_mapping,
        }],
        None,
    )
    .expect("view")
    .with_semantic_access(access)
    .expect("authorization")
}
fn access() -> SemanticAccess {
    SemanticAccess {
        spaces: None,
        contexts: None,
        sources: SourceFilter::default(),
    }
}
struct NoText;
impl EvidenceProvider<()> for NoText {
    fn read_evidence(
        &self,
        _: &KgReadView<()>,
        refs: &[EvidenceRef],
        _: &RequestContext,
    ) -> QueryResult<Vec<EvidenceRead>> {
        Ok(refs
            .iter()
            .map(|reference| EvidenceRead::Unavailable {
                reference: reference.clone(),
                reason: "not_retained".into(),
            })
            .collect())
    }
}
fn stored(
    data: &FactCatalog,
) -> Result<(tempfile::TempDir, GraphDbStore, String), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let (_, mut spec) = super::tests::build_fixture()?;
    spec.base_catalog_digest = stable_digest(&"semantic-text")?;
    spec.policy_fingerprint.clone_from(&data.config_fingerprint);
    spec.configuration = serde_json::json!({});
    let path = dir.path().join("graph");
    let store = GraphDbStore::create_batched_build(&path, GraphLimits::default())?;
    store.write_facts_batched(data, &ImportBudget::default(), &context())?;
    store.commit_build(&spec, &context())?;
    store.checkpoint()?;
    drop(store);
    let store = GraphDbStore::open(&path, true, GraphLimits::default())?;
    assert_eq!(
        store.read_build_facts(&spec, &context())?,
        Some(data.clone())
    );
    Ok((dir, store, spec.graph_build_id))
}
fn facts(result: &ExploreResult) -> Vec<(&str, FactState)> {
    result
        .semantic
        .as_ref()
        .expect("semantic result")
        .objects
        .iter()
        .filter_map(|object| match &object.record {
            SemanticValue::Fact(fact) => Some((fact.identity.id.as_str(), fact.state)),
            _ => None,
        })
        .collect()
}
#[test]
fn semantic_correction_queries_reopen_and_never_restore_a_predecessor() -> TestResult {
    let corrected = base().apply_sources(&input("explicit-correction"), &context())?;
    let mut deletion = input("explicit-correction");
    deletion.sources = vec![SourceChange::Delete {
        source: deletion.sources[0].source().clone(),
    }];
    deletion.fact_changes.clear();
    let removed = corrected.clone().apply_sources(&deletion, &context())?;
    let replayed = removed
        .clone()
        .apply_sources(&input("explicit-correction"), &context())?;
    for (catalog, expected) in [
        (corrected, vec![("f2", FactState::Active)]),
        (removed, vec![]),
        (replayed, vec![]),
    ] {
        let (_dir, reader, id) = stored(&catalog)?;
        let view = view(&id, access());
        let current = explore(&reader, &NoText, &view, &request(false), &ctx())?;
        assert_eq!(facts(&current), expected);
        let history = explore(&reader, &NoText, &view, &request(true), &ctx())?;
        assert_eq!(
            facts(&history),
            [("f1", FactState::Superseded), ("f2", FactState::Active)]
        );
        assert_eq!(history.semantic.expect("history").changes.len(), 1);
    }
    Ok(())
}
#[test]
fn semantic_authorization_intersects_sources_context_space_and_view() -> TestResult {
    let data = base().apply_sources(&input("explicit-correction"), &context())?;
    let (_dir, reader, id) = stored(&data)?;
    let grant = SemanticAccess {
        sources: SourceFilter {
            kinds: None,
            sources: Some(vec!["a.md".into()]),
        },
        ..access()
    };
    let restricted = view(&id, grant);
    assert!(
        facts(&explore(
            &reader,
            &NoText,
            &restricted,
            &request(false),
            &ctx()
        )?)
        .is_empty()
    );
    let history = explore(&reader, &NoText, &restricted, &request(true), &ctx())?;
    assert_eq!(facts(&history), [("f1", FactState::Superseded)]);
    assert!(!serde_json::to_string(&history)?.contains("c.md"));
    let no_context = view(
        &id,
        SemanticAccess {
            contexts: Some(vec!["private".into()]),
            ..access()
        },
    );
    assert!(
        explore(&reader, &NoText, &no_context, &request(true), &ctx())?
            .semantic
            .expect("semantic")
            .objects
            .is_empty()
    );
    let no_space = view(
        &id,
        SemanticAccess {
            spaces: Some(vec!["other".into()]),
            ..access()
        },
    );
    assert!(
        explore(&reader, &NoText, &no_space, &request(true), &ctx())?
            .semantic
            .expect("semantic")
            .objects
            .is_empty()
    );
    let mut stale = request(false);
    if let ExploreOperation::Semantic { query } = &mut stale.operation {
        query.anchor = Some(ObjectRef {
            generation: "generation".into(),
            workspace_id: "workspace".into(),
            graph_build_id: "old".into(),
            object: ObjectId::Fact {
                identity: Identity {
                    space_id: "workspace".into(),
                    id: "f2".into(),
                },
            },
        });
    }
    assert_eq!(
        explore(&reader, &NoText, &restricted, &stale, &ctx())
            .expect_err("stale object")
            .kind,
        QueryErrorKind::ViewMismatch
    );
    let signal = tokio_util::sync::CancellationToken::new();
    let cancelled = RequestContext::new(
        BudgetLimits::default(),
        signal.clone(),
        Instant::now() + Duration::from_secs(30),
    )?;
    signal.cancel();
    assert!(explore(&reader, &NoText, &restricted, &request(false), &cancelled).is_err());
    Ok(())
}

const TEXT: &str = "# Project\n";
fn mapped() -> Result<FactCatalog, Box<dyn std::error::Error>> {
    let facts =
        crate::relations::extract_markdown(super::tests::version("a.md", TEXT), TEXT, &context())?;
    let mut file = super::tests::bind(facts, TEXT, 1)?;
    let entity_id = file.node_entities[0][0].clone();
    file.text_map.push(TextMap {
        source_id: "a.md".into(),
        source_span: ByteSpan {
            start: 0,
            end: TEXT.len() as u64,
        },
        entity_id,
        content_span: ByteSpan {
            start: 0,
            end: TEXT.len() as u64,
        },
    });
    file.text_binding_digest = Some(stable_digest(&"mapped")?);
    let original = base();
    let mut data = original.semantic;
    let version = zg_storage::sha256_hex(TEXT.as_bytes());
    data.sources[0].source.source_version.clone_from(&version);
    data.sources[0].semantic_version = Some(version.clone());
    for association in &mut data.associations {
        association
            .evidence
            .source
            .source_version
            .clone_from(&version);
        association.evidence.locator = Some(EvidenceRef {
            source_id: "a.md".into(),
            raw_digest: version.clone(),
            decoded_digest: version.clone(),
            span: ByteSpan {
                start: 0,
                end: TEXT.len() as u64,
            },
            reference_id: None,
        });
    }
    data.normalize()?;
    Ok(FactCatalog::resolve(
        vec![file],
        original.config_fingerprint,
        original.selection_fingerprint,
        &context(),
    )?
    .with_semantic(data, &context())?)
}
struct Text {
    calls: std::cell::Cell<usize>,
}
impl EvidenceProvider<()> for Text {
    fn read_evidence(
        &self,
        _: &KgReadView<()>,
        refs: &[EvidenceRef],
        _: &RequestContext,
    ) -> QueryResult<Vec<EvidenceRead>> {
        refs.iter()
            .map(|reference| {
                assert_eq!(
                    reference.raw_digest,
                    zg_storage::sha256_hex(TEXT.as_bytes())
                );
                Ok(EvidenceRead::Verified {
                    reference: reference.clone(),
                    content: reference.span.slice(TEXT)?.into(),
                })
            })
            .collect()
    }
}
impl ScopeTextProvider for Text {
    fn text_entities(
        &self,
        ids: &[String],
        _: &RequestContext,
    ) -> QueryResult<Vec<ScopeTextEntity>> {
        self.calls.set(self.calls.get() + 1);
        Ok(ids
            .iter()
            .map(|id| ScopeTextEntity {
                id: id.clone(),
                content_bytes: TEXT.len() as u64,
                fragments: vec![ScopeTextFragment {
                    id: "fragment".into(),
                    span: ByteSpan {
                        start: 0,
                        end: TEXT.len() as u64,
                    },
                }],
            })
            .collect())
    }
}
#[test]
fn semantic_scope_reuses_real_rows_and_rejects_missing_or_partial_mapping() -> TestResult {
    let data = mapped()?;
    let (_dir, reader, id) = stored(&data)?;
    let text = Text {
        calls: std::cell::Cell::new(0),
    };
    assert_eq!(
        resolve_semantic_scope(
            &reader,
            &text,
            &text,
            &view(&id, access()),
            &request(false),
            &ctx()
        )
        .expect_err("mapping capability required")
        .kind,
        QueryErrorKind::GraphUnavailable
    );
    assert_eq!(text.calls.get(), 0);
    let view = granted_view(&id, access(), true);
    let scope = resolve_semantic_scope(&reader, &text, &text, &view, &request(false), &ctx())?;
    assert_eq!(scope.units().len(), 1);
    assert_eq!(scope.units()[0].fragment_id, "fragment");
    assert!(
        resolve_semantic_scope(&reader, &NoText, &text, &view, &request(false), &ctx()).is_err()
    );
    let mut empty = request(false);
    empty.boundary.sources = Some(vec![]);
    let empty_text = Text {
        calls: std::cell::Cell::new(0),
    };
    assert!(
        resolve_semantic_scope(&reader, &empty_text, &empty_text, &view, &empty, &ctx())?
            .is_empty()
    );
    assert_eq!(empty_text.calls.get(), 0);
    let limited = RequestContext::new(
        BudgetLimits {
            output_bytes: 1024,
            ..Default::default()
        },
        tokio_util::sync::CancellationToken::new(),
        Instant::now() + Duration::from_secs(30),
    )?;
    assert!(
        resolve_semantic_scope(&reader, &text, &text, &view, &request(false), &limited).is_err()
    );
    for partial in [false, true] {
        let mut missing = data.clone();
        if partial {
            missing.files[0].text_map[0].source_span.end -= 1;
            missing.files[0].text_map[0].content_span.end -= 1;
        } else {
            missing.files[0].text_map.clear();
        }
        let (_dir, reader, id) = stored(&missing)?;
        assert!(
            resolve_semantic_scope(
                &reader,
                &text,
                &text,
                &granted_view(&id, access(), true),
                &request(false),
                &ctx()
            )
            .is_err()
        );
    }
    Ok(())
}
#[test]
#[expect(
    clippy::too_many_lines,
    reason = "one verified document source covers semantic selection and both adjacency directions"
)]
fn semantic_mentions_traverse_document_source_nodes_without_auxiliary_hops() -> TestResult {
    let mut data = mapped()?;
    let owner = data.files[0].facts.as_ref().expect("facts").nodes[0]
        .id
        .clone();
    let mut evidence = data.semantic.associations[0]
        .evidence
        .locator
        .clone()
        .expect("position");
    evidence.reference_id = Some("mention".into());
    data.semantic.mentions.push(Mention {
        ref_id: "mention".into(),
        owner_id: owner.clone(),
        target: Identity {
            space_id: "workspace".into(),
            id: "project-a".into(),
        },
        evidence,
    });
    data.semantic.normalize()?;
    let (_dir, reader, id) = stored(&data)?;
    let mut query = request(false);
    if let ExploreOperation::Semantic { query } = &mut query.operation {
        query.anchor = Some(ObjectRef {
            generation: "generation".into(),
            workspace_id: "workspace".into(),
            graph_build_id: id.clone(),
            object: ObjectId::SourceNode {
                node_id: owner.clone(),
            },
        });
    }
    let output = explore(
        &reader,
        &Text {
            calls: std::cell::Cell::new(0),
        },
        &view(&id, access()),
        &query,
        &ctx(),
    )?;
    let result = output.semantic.expect("semantic");
    assert_eq!(result.mentions.len(), 1);
    assert!(result.objects.iter().any(
        |o| matches!(&o.reference.object, ObjectId::SourceNode { node_id } if node_id == &owner)
    ));
    assert!(result.objects.iter().any(|o| matches!(&o.reference.object, ObjectId::SemanticEntity { identity } if identity.id == "project-a")));
    let mut adjacency = request(false);
    adjacency.operation = ExploreOperation::Neighbors {
        anchor: Locator::Object {
            object: ObjectRef {
                generation: "generation".into(),
                workspace_id: "workspace".into(),
                graph_build_id: id.clone(),
                object: ObjectId::SourceNode {
                    node_id: owner.clone(),
                },
            },
        },
        relations: vec![RelationKind::Mentions],
        direction: Direction::Outgoing,
    };
    adjacency.result_filter.kinds = Some(vec!["project".into()]);
    let page = explore(
        &reader,
        &Text {
            calls: std::cell::Cell::new(0),
        },
        &view(&id, access()),
        &adjacency,
        &ctx(),
    )?;
    assert_eq!(page.relations.len(), 1);
    assert!(page.relations[0].mention.is_some());
    let node_kind = data.files[0].facts.as_ref().expect("facts").nodes[0].entity_kind;
    adjacency.operation = ExploreOperation::Neighbors {
        anchor: Locator::Object {
            object: ObjectRef {
                generation: "generation".into(),
                workspace_id: "workspace".into(),
                graph_build_id: id.clone(),
                object: ObjectId::SemanticEntity {
                    identity: Identity {
                        space_id: "workspace".into(),
                        id: "project-a".into(),
                    },
                },
            },
        },
        relations: vec![RelationKind::Mentions],
        direction: Direction::Incoming,
    };
    adjacency.result_filter.kinds = Some(vec![
        serde_json::to_value(node_kind)?
            .as_str()
            .expect("kind")
            .into(),
    ]);
    assert_eq!(
        explore(
            &reader,
            &Text {
                calls: std::cell::Cell::new(0)
            },
            &view(&id, access()),
            &adjacency,
            &ctx()
        )?
        .relations
        .len(),
        1
    );
    Ok(())
}
#[test]
fn semantic_history_does_not_return_unverified_positions_or_withdrawn_episodes_as_current()
-> TestResult {
    let mut data = mapped()?;
    let mut retract = input("explicit-retraction");
    retract.update_scope = vec!["a.md".into()];
    retract.sources.clear();
    if let FactChange::Retract { evidence, .. } = &mut retract.fact_changes[0] {
        *evidence = data.semantic.associations[0].evidence.clone();
    }
    data = data.apply_sources(&retract, &context())?;
    let (_dir, reader, id) = stored(&data)?;
    let output = explore(
        &reader,
        &NoText,
        &view(&id, access()),
        &request(true),
        &ctx(),
    )?;
    let result = output.semantic.expect("history");
    assert!(
        result
            .objects
            .iter()
            .flat_map(|o| &o.provenance)
            .all(|p| p.evidence.locator.is_none() && p.read.is_none())
    );
    assert!(
        result
            .changes
            .iter()
            .all(|change| change.evidence().locator.is_none())
    );
    assert!(!serde_json::to_string(&result)?.contains("\"span\""));
    let mut initial = input("initial");
    initial.episodes.push(Episode {
        identity: Identity {
            space_id: "workspace".into(),
            id: "event".into(),
        },
        kind: "meeting".into(),
        source: initial.sources[0].source().clone(),
        context: "project-a".into(),
        occurred_at_ms: None,
        ingested_at_ms: 1,
        content_ref: "host:event".into(),
        state: EpisodeState::Active,
    });
    let blank = FactCatalog::resolve(
        vec![],
        stable_digest(&serde_json::json!({}))?,
        stable_digest(&"selection")?,
        &context(),
    )?;
    let mut state = blank.apply_sources(&initial, &context())?;
    initial.sources.clear();
    initial.episodes.clear();
    initial.episode_withdrawals = vec![EpisodeWithdrawal {
        target: Identity {
            space_id: "workspace".into(),
            id: "event".into(),
        },
        expected_state: EpisodeState::Active,
        context: "project-a".into(),
        changed_at_ms: 2,
        evidence: SourceEvidence {
            episode: None,
            source: state.semantic.sources[0].source.clone(),
            locator: None,
        },
    }];
    state = state.apply_sources(&initial, &context())?;
    let (_dir, reader, id) = stored(&state)?;
    let mut current = request(false);
    if let ExploreOperation::Semantic { query } = &mut current.operation {
        query.relation_type = None;
    }
    assert!(
        explore(&reader, &NoText, &view(&id, access()), &current, &ctx())?
            .semantic
            .expect("current")
            .objects
            .iter()
            .all(|o| !matches!(o.record, SemanticValue::Episode(_)))
    );
    if let ExploreOperation::Semantic { query } = &mut current.operation {
        query.history = true;
    }
    assert!(explore(&reader, &NoText, &view(&id, access()), &current, &ctx())?.semantic.expect("history").objects.iter().any(|o| matches!(&o.record, SemanticValue::Episode(e) if e.state == EpisodeState::Withdrawn)));
    Ok(())
}
#[test]
fn semantic_scalar_pages_and_output_budget_preserve_explicit_completion() -> TestResult {
    let mut initial = input("initial");
    if let SourceChange::Upsert {
        semantic: Some(SemanticUpdate::Complete(batch)),
        ..
    } = &mut initial.sources[0]
    {
        for i in 0..260 {
            let mut fact = batch.facts[0].clone();
            fact.identity.id = format!("scalar-{i:03}");
            fact.object = FactObject::Scalar {
                value: ScalarValue::Number(i.into()),
            };
            batch.facts.push(fact);
        }
    }
    let data = base().apply_sources(&initial, &context())?;
    let (_dir, reader, id) = stored(&data)?;
    assert_eq!(
        reader
            .semantic_page(SemanticPartition::ScalarFacts, "workspace", 0, &ctx())?
            .facts
            .len(),
        256
    );
    assert_eq!(
        reader
            .semantic_page(SemanticPartition::ScalarFacts, "workspace", 256, &ctx())?
            .facts
            .len(),
        4
    );
    let output = explore(
        &reader,
        &NoText,
        &view(&id, access()),
        &request(false),
        &ctx(),
    )?;
    assert_eq!(output.execution.status, Completion::Partial);
    assert_eq!(
        output.execution.stop_reason.as_deref(),
        Some("output_budget")
    );
    assert!(
        u64::try_from(serde_json::to_vec(&output)?.len())? <= BudgetLimits::default().output_bytes
    );
    Ok(())
}

#[test]
fn implementation_reference_pages_filter_declaration_files_before_logical_offsets() -> TestResult {
    use crate::relations::{
        Binding, BindingTarget, CodeModule, FileFacts, Language, Lookup, RelationKind, Span,
    };
    let (original, _) = super::semantic_tests::semantic_fixture()?;
    let mut files = original.files;
    for (path, count, file_id) in [("left.rs", 270, 10), ("right.rs", 20, 11)] {
        let text = "impl Child for Impl {}\n".repeat(count);
        let mut facts = FileFacts::new(
            super::tests::version(path, &text),
            Language::Rust,
            Some(CodeModule {
                namespace: "fixture".into(),
                path: vec![path.into()],
                is_package: false,
            }),
            &text,
        )?;
        for name in ["Child", "Impl"] {
            facts.bindings.push(Binding {
                scope: 0,
                name: name.into(),
                target: BindingTarget::Import {
                    path: vec!["crate".into(), "types".into(), name.into()],
                },
                visible_from: 0,
                visibility: String::new(),
            });
        }
        for i in 0..count {
            let start = i * "impl Child for Impl {}\n".len() + 5;
            facts.add_reference(
                0,
                RelationKind::Implements,
                Span {
                    start,
                    end: start + 5,
                },
                Lookup::TypeRelation {
                    owner: vec!["Impl".into()],
                    target: vec!["Child".into()],
                },
            )?;
        }
        files.push(super::tests::bind(facts, &text, file_id)?);
    }
    let data = FactCatalog::resolve(
        files,
        stable_digest(&serde_json::json!({}))?,
        original.selection_fingerprint,
        &context(),
    )?
    .with_semantic(original.semantic, &context())?;
    let (_dir, reader, _) = stored(&data)?;
    let budget = ctx();
    let first = reader.references(
        ReferencePartition::Implements,
        None,
        Some("left.rs"),
        None,
        0,
        &budget,
    )?;
    let second = reader.references(
        ReferencePartition::Implements,
        None,
        Some("left.rs"),
        None,
        256,
        &budget,
    )?;
    assert_eq!(first.len(), 256);
    assert_eq!(second.len(), 14);
    let ids = first
        .iter()
        .chain(&second)
        .map(|r| r.id.as_str())
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(ids.len(), 270);
    assert!(first.iter().chain(&second).all(|r| {
        r.evidence.source_id == "left.rs"
            && data
                .files
                .iter()
                .find_map(|f| f.facts.as_ref().filter(|f| f.version.path == "types.rs"))
                .expect("types")
                .nodes
                .iter()
                .any(|n| n.id == r.owner && n.name == "Impl")
    }));
    Ok(())
}

#[test]
#[expect(
    clippy::too_many_lines,
    reason = "one native fixture verifies stable continuation and authorization changes"
)]
fn native_semantic_keyset_pages_keep_parallel_facts_self_loops_and_authorization() -> TestResult {
    let mut catalog = base();
    let original = catalog.semantic.facts[0].clone();
    let support = catalog
        .semantic
        .associations
        .iter()
        .find(|a| {
            a.target
                == ObjectId::Fact {
                    identity: original.identity.clone(),
                }
                && a.role == SourceRole::Support
        })
        .expect("support")
        .clone();
    for (id, predicate, self_loop) in [("parallel", "owner", false), ("self", "works_on", true)] {
        let mut fact = original.clone();
        fact.identity.id = id.into();
        fact.relation_type = predicate.into();
        if self_loop {
            fact.object = FactObject::Entity {
                identity: fact.subject.clone(),
            };
        }
        let mut contribution = support.clone();
        contribution.target = ObjectId::Fact {
            identity: fact.identity.clone(),
        };
        catalog.semantic.facts.push(fact);
        catalog.semantic.associations.push(contribution);
    }
    catalog.semantic.normalize()?;
    let (_dir, reader, id) = stored(&catalog)?;
    let read_view = view(&id, access());
    let anchor = Locator::Object {
        object: ObjectRef {
            generation: "generation".into(),
            workspace_id: "workspace".into(),
            graph_build_id: id.clone(),
            object: ObjectId::SemanticEntity {
                identity: original.subject.clone(),
            },
        },
    };
    let mut req = request(false);
    req.operation = ExploreOperation::Neighbors {
        anchor: anchor.clone(),
        relations: vec![RelationKind::SemanticRelation],
        direction: Direction::Both,
    };
    req.page_size = 1;
    let mut found = Vec::new();
    let mut first_cursor = None;
    loop {
        let result = explore(&reader, &NoText, &read_view, &req, &ctx())?;
        found.extend(
            result
                .relations
                .iter()
                .map(|edge| edge.fact.as_ref().expect("fact").identity.id.clone()),
        );
        if first_cursor.is_none() {
            first_cursor = result.next_cursor.clone();
        }
        req.cursor = result.next_cursor;
        if req.cursor.is_none() {
            break;
        }
    }
    found.sort();
    assert_eq!(found, ["f1", "parallel", "self"]);
    req.cursor = first_cursor;
    let restricted = view(
        &id,
        SemanticAccess {
            sources: SourceFilter {
                sources: Some(vec![]),
                kinds: None,
            },
            ..access()
        },
    );
    assert!(
        explore(&reader, &NoText, &restricted, &req, &ctx()).is_err(),
        "authorization changed"
    );
    req.cursor = None;
    req.relation_filters = vec![RelationSelector {
        edge_type: RelationKind::SemanticRelation,
        relation_types: Some(vec!["works_on".into()]),
    }];
    let only_self = explore(&reader, &NoText, &read_view, &req, &ctx())?;
    assert_eq!(only_self.relations.len(), 1);
    assert_eq!(
        only_self.relations[0]
            .fact
            .as_ref()
            .expect("valid fixture/result")
            .identity
            .id,
        "self"
    );
    req.operation = ExploreOperation::RelationTypes {
        anchor,
        direction: Direction::Both,
    };
    req.relation_filters[0].relation_types = None;
    let mut types = Vec::new();
    loop {
        let result = explore(&reader, &NoText, &read_view, &req, &ctx())?;
        types.extend(result.relation_types);
        req.cursor = result.next_cursor;
        if req.cursor.is_none() {
            break;
        }
    }
    assert_eq!(
        types.len(),
        3,
        "one incoming self-loop type, two outgoing types"
    );
    Ok(())
}

#[test]
fn episode_links_round_trip_and_leave_only_current_endpoints_visible() -> TestResult {
    let mut catalog = base();
    let source = catalog.semantic.sources[0].source.clone();
    for id in ["request", "result"] {
        catalog.semantic.episodes.push(Episode {
            identity: Identity {
                space_id: "workspace".into(),
                id: id.into(),
            },
            kind: "message".into(),
            source: source.clone(),
            context: "project-a".into(),
            occurred_at_ms: None,
            ingested_at_ms: 100,
            content_ref: id.into(),
            state: EpisodeState::Active,
        });
    }
    catalog.semantic.episode_links.push(EpisodeLink {
        identity: Identity {
            space_id: "workspace".into(),
            id: "result-request".into(),
        },
        source: catalog.semantic.episodes[1].identity.clone(),
        target: catalog.semantic.episodes[0].identity.clone(),
        relation_type: EpisodeRelation::ResultOf,
    });
    catalog.semantic.normalize()?;
    let (_dir, reader, id) = stored(&catalog)?;
    let mut req = request(false);
    req.operation = ExploreOperation::Neighbors {
        anchor: Locator::Object {
            object: ObjectRef {
                generation: "generation".into(),
                workspace_id: "workspace".into(),
                graph_build_id: id.clone(),
                object: ObjectId::Episode {
                    identity: Identity {
                        space_id: "workspace".into(),
                        id: "result".into(),
                    },
                },
            },
        },
        relations: vec![RelationKind::EpisodeLink],
        direction: Direction::Outgoing,
    };
    let result = explore(&reader, &NoText, &view(&id, access()), &req, &ctx())?;
    assert_eq!(result.relations.len(), 1);
    assert_eq!(
        result.relations[0]
            .episode_link
            .as_ref()
            .expect("valid fixture/result")
            .relation_type,
        EpisodeRelation::ResultOf
    );
    assert!(
        explore(
            &reader,
            &NoText,
            &view(
                &id,
                SemanticAccess {
                    contexts: Some(vec!["other".into()]),
                    ..access()
                }
            ),
            &req,
            &ctx()
        )?
        .relations
        .is_empty()
    );
    Ok(())
}

#[test]
fn native_cursor_never_exposes_a_filtered_fact_key() -> TestResult {
    let mut catalog = base();
    let original = catalog.semantic.facts[0].clone();
    let contribution = catalog
        .semantic
        .associations
        .iter()
        .find(|a| {
            a.target
                == ObjectId::Fact {
                    identity: original.identity.clone(),
                }
        })
        .expect("support")
        .clone();
    catalog.semantic.sources.push(SemanticSource {
        source: SourceRef {
            source_id: "private".into(),
            source_version: "v1".into(),
        },
        state: SemanticSourceState::Complete,
        semantic_version: Some("v1".into()),
    });
    for (id, kind, private) in [
        ("second", "owner", false),
        ("secret-identity", "owner_private", true),
        ("last", "works_on", false),
    ] {
        let mut fact = original.clone();
        fact.identity.id = id.into();
        fact.relation_type = kind.into();
        let mut support = contribution.clone();
        support.target = ObjectId::Fact {
            identity: fact.identity.clone(),
        };
        if private {
            support.evidence.source.source_id = "private".into();
        }
        catalog.semantic.facts.push(fact);
        catalog.semantic.associations.push(support);
    }
    catalog.semantic.normalize()?;
    let (_dir, reader, id) = stored(&catalog)?;
    let grant = view(
        &id,
        SemanticAccess {
            sources: SourceFilter {
                sources: Some(vec!["a.md".into()]),
                kinds: None,
            },
            ..access()
        },
    );
    let mut req = request(false);
    req.page_size = 1;
    req.operation = ExploreOperation::Neighbors {
        anchor: Locator::Object {
            object: ObjectRef {
                generation: "generation".into(),
                workspace_id: "workspace".into(),
                graph_build_id: id,
                object: ObjectId::SemanticEntity {
                    identity: original.subject,
                },
            },
        },
        relations: vec![RelationKind::SemanticRelation],
        direction: Direction::Outgoing,
    };
    let mut found = Vec::new();
    loop {
        let result = explore(&reader, &NoText, &grant, &req, &ctx())?;
        let serialized = serde_json::to_string(&result)?;
        assert!(!serialized.contains("secret-identity"));
        assert!(!serialized.contains("owner_private"));
        found.extend(
            result
                .relations
                .iter()
                .map(|edge| edge.fact.as_ref().expect("fact").identity.id.clone()),
        );
        req.cursor = result.next_cursor;
        if req.cursor.is_none() {
            break;
        }
    }
    assert_eq!(found, ["f1", "second", "last"]);
    Ok(())
}
