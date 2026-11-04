use super::tests::{build_fixture, context};
use super::*;
use crate::relations::{
    Binding, BindingTarget, CodeModule, EntityKind, FactNode, FileFacts, Language, Lookup,
    NodeKind, RelationKind, Scope, ScopeKind, Span, node_id,
};
use crate::{
    query::{ByteSpan, EvidenceRef},
    semantic::*,
};
use serde_json::json;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;
fn id(value: &str) -> Identity {
    Identity {
        space_id: "workspace".into(),
        id: value.into(),
    }
}
fn source(value: &str) -> SourceRef {
    SourceRef {
        source_id: value.into(),
        source_version: "v1".into(),
    }
}
fn current(value: &str) -> SemanticSource {
    SemanticSource {
        source: source(value),
        state: SemanticSourceState::Complete,
        semantic_version: Some("v1".into()),
    }
}
pub(super) fn semantic_fixture() -> TestResult<(FactCatalog, crate::graph::build::BuildSpec)> {
    let (original, mut spec) = build_fixture()?;
    let mut files = original.files;
    files.push(declared_type_relations()?);
    for (path, text, file_id) in [
        ("unique.md", "# Unique\n", 6),
        ("link.md", "[unique](unique.md#unique)\n", 7),
    ] {
        let facts = crate::relations::extract_markdown(
            super::tests::version(path, text),
            text,
            &context(),
        )?;
        files.push(super::tests::bind(facts, text, file_id)?);
    }
    let code = FactCatalog::resolve(
        files,
        original.config_fingerprint,
        original.selection_fingerprint,
        &context(),
    )?;
    spec.base_catalog_digest = crate::processing::stable_digest(
        &code
            .files
            .iter()
            .map(|file| &file.record)
            .collect::<Vec<_>>(),
    )?;
    let file = code
        .files
        .iter()
        .find_map(|f| f.facts.as_ref().filter(|f| f.version.path == "a.py"))
        .ok_or("source")?;
    let evidence = EvidenceRef {
        source_id: "a.py".into(),
        raw_digest: file.version.raw_digest.clone(),
        decoded_digest: file.version.decoded_digest.clone(),
        span: ByteSpan { start: 0, end: 6 },
        reference_id: Some("semantic-mention".into()),
    };
    let mut data = semantic_records()?;
    data.mentions.push(Mention {
        ref_id: "semantic-mention".into(),
        owner_id: file.nodes[0].id.clone(),
        target: id("person"),
        evidence,
    });
    for (index, value) in [
        json!("中文"),
        json!(u64::MAX),
        json!(0.5),
        json!(true),
        json!(null),
    ]
    .into_iter()
    .enumerate()
    {
        data.facts.push(Fact {
            identity: id(&format!("scalar-{index}")),
            subject: id("project"),
            predicate: format!("attribute-{index}"),
            object: FactObject::Scalar {
                value: serde_json::from_value(value)?,
            },
            context: "project".into(),
            state: FactState::Active,
            supersedes: None,
            changed_at_ms: 100,
            attributes: serde_json::Map::new(),
        });
    }
    for fact in &data.facts {
        data.associations.push(SourceAssociation {
            target: ObjectId::Fact {
                identity: fact.identity.clone(),
            },
            evidence: SourceEvidence {
                source: source("a.py"),
                locator: None,
            },
            role: SourceRole::Support,
        });
    }
    data.associations.push(SourceAssociation {
        target: ObjectId::SemanticEntity {
            identity: id("person"),
        },
        evidence: SourceEvidence {
            source: source("a.py"),
            locator: Some(data.mentions[0].evidence.clone()),
        },
        role: SourceRole::Support,
    });
    Ok((code.with_semantic(data, &context())?, spec))
}

fn semantic_records() -> TestResult<SemanticCatalog> {
    Ok(SemanticCatalog {
        sources: vec![current("a.py"), current("episode:meeting")],
        entities: vec![
            SemanticEntity {
                identity: id("project"),
                kind: "project".into(),
                name: "项目 A".into(),
                aliases: vec!["A".into()],
                attributes: serde_json::from_value(
                    json!({"active":true,"weight":0.5,"tags":["中",3],"id":"business property"}),
                )?,
            },
            SemanticEntity {
                identity: id("person"),
                kind: "person".into(),
                name: "张三".into(),
                aliases: vec!["Zhang".into()],
                attributes: serde_json::Map::new(),
            },
            SemanticEntity {
                identity: Identity {
                    space_id: "other".into(),
                    id: "person".into(),
                },
                kind: "person".into(),
                name: "张三".into(),
                aliases: vec![],
                attributes: serde_json::Map::new(),
            },
        ],
        episodes: vec![Episode {
            identity: id("person"),
            kind: "meeting".into(),
            source: source("episode:meeting"),
            context: "project".into(),
            occurred_at_ms: Some(1_700_000_000_123),
            ingested_at_ms: 1_700_000_001_234,
            content_ref: "host:record-1".into(),
            state: EpisodeState::Active,
        }],
        facts: vec![Fact {
            identity: id("f1"),
            subject: id("project"),
            predicate: "owner".into(),
            object: FactObject::Entity {
                identity: id("person"),
            },
            context: "project".into(),
            state: FactState::Active,
            supersedes: None,
            changed_at_ms: 1_700_000_000_123,
            attributes: serde_json::from_value(
                json!({"confidence":0.75,"verified":false,"tags":[1,"a"]}),
            )?,
        }],
        associations: vec![],
        mentions: vec![],
        deferred_mentions: vec![],
        changes: vec![],
        last_operation: None,
    })
}

fn declared_type_relations() -> TestResult<crate::relations::bound::BoundFileFacts> {
    let text = "trait Base {}\ntrait Child: Base {}\nstruct Impl;\nimpl Child for Impl {}\nuse self::Base as Alias;\n";
    let mut facts = FileFacts::new(
        super::tests::version("types.rs", text),
        Language::Rust,
        Some(CodeModule {
            namespace: "fixture".into(),
            path: vec!["types".into()],
            is_package: false,
        }),
        text,
    )?;
    for (name, kind, marker) in [
        ("Base", EntityKind::Interface, "trait Base"),
        ("Child", EntityKind::Interface, "trait Child"),
        ("Impl", EntityKind::Type, "struct Impl"),
    ] {
        let start = text.find(marker).ok_or("declaration")?;
        let end = start + text[start..].find('\n').ok_or("line")?;
        let id = node_id("types.rs", Language::Rust, "type", &[name.into()], 0)?;
        facts.nodes.push(FactNode {
            id: id.clone(),
            kind: NodeKind::Symbol,
            entity_kind: kind,
            name: name.into(),
            qualified_name: vec![name.into()],
            parent: Some(facts.nodes[0].id.clone()),
            path_addressable: true,
            callable: false,
            span: Span { start, end },
            entity_indices: vec![],
        });
        facts.bindings.push(Binding {
            scope: 0,
            name: name.into(),
            target: BindingTarget::Node { id: id.clone() },
            visible_from: 0,
            visibility: "pub".into(),
        });
        facts.scopes.push(Scope {
            parent: Some(0),
            owner: id,
            kind: ScopeKind::Class,
        });
    }
    let base = text.find(": Base").ok_or("base reference")? + 2;
    facts.add_reference(
        2,
        RelationKind::Extends,
        Span {
            start: base,
            end: base + 4,
        },
        Lookup::Name {
            parts: vec!["Base".into()],
        },
    )?;
    let child = text.find("impl Child").ok_or("impl reference")? + 5;
    facts.add_reference(
        3,
        RelationKind::Implements,
        Span {
            start: child,
            end: child + 5,
        },
        Lookup::Name {
            parts: vec!["Child".into()],
        },
    )?;
    let imported = text.rfind("Base").ok_or("import reference")?;
    facts.add_reference(
        0,
        RelationKind::Import,
        Span {
            start: imported,
            end: imported + 4,
        },
        Lookup::Import {
            path: vec!["self".into(), "Base".into()],
        },
    )?;
    Ok(super::tests::bind(facts, text, 5)?)
}

#[test]
fn semantic_records_round_trip_and_survive_checkpoint_and_readonly_reopen() -> TestResult {
    let (catalog, spec) = semantic_fixture()?;
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("graph");
    let store = GraphDbStore::create_build(&path, GraphLimits::default())?;
    let counts = store.write_facts(&catalog, &context())?;
    assert_eq!(counts.nodes["KgSemanticEntity"], 3);
    assert_eq!(counts.nodes["KgEpisode"], 1);
    assert_eq!(counts.nodes["KgScalarFact"], 5);
    assert_eq!(counts.relations["KgSemanticRelation"], 1);
    assert_eq!(counts.relations["KgMentions"], 1);
    for relation in [
        "KgContains",
        "KgCalls",
        "KgImports",
        "KgLinks",
        "KgExtends",
        "KgImplements",
        "KgMentions",
        "KgSemanticRelation",
    ] {
        assert!(counts.relations[relation] > 0, "{relation}");
    }
    assert_eq!(store.read_facts(&context())?, catalog);
    assert_eq!(
        codec::catalog_digest(&catalog)?,
        crate::processing::stable_digest(&catalog)?
    );
    let rows = codec::FactRows::encode(&catalog)?;
    let mut previous = crate::processing::stable_digest(&"kg-rows-v7")?;
    let mut scratch = Vec::new();
    for mutation in rows.mutations() {
        let expected = crate::processing::canonical_json(&(&previous, &mutation))?;
        let next = mutation.update_digest(&previous, &mut scratch)?;
        assert_eq!(scratch, expected.as_bytes());
        assert_eq!(
            next,
            crate::processing::stable_digest(&(&previous, &mutation))?
        );
        previous = next;
    }
    assert_eq!(store.verify_rows(&context())?, rows.digest()?);
    let proof = store.commit_build(&spec, &context())?;
    store.checkpoint()?;
    drop(store);
    let reopened = GraphDbStore::open(&path, true, GraphLimits::default())?;
    assert_eq!(reopened.read_facts(&context())?, catalog);
    assert_eq!(reopened.counts()?, counts);
    assert_eq!(reopened.inspect_build(&spec, &context())?, Some(proof));
    drop(reopened);
    let writable = GraphDbStore::open(&path, false, GraphLimits::default())?;
    assert_eq!(writable.read_facts(&context())?.semantic, catalog.semantic);
    Ok(())
}

#[test]
fn semantic_batches_keep_large_json_and_reuse_the_existing_import_receipt() -> TestResult {
    let (mut catalog, spec) = semantic_fixture()?;
    catalog.semantic.entities[0]
        .attributes
        .insert("large".into(), json!("中".repeat(50_000)));
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("graph");
    let store = GraphDbStore::create_batched_build(&path, GraphLimits::default())?;
    let budget = ImportBudget {
        max_batch_rows: 2,
        max_batch_bytes: 256 * 1024,
    };
    let first = store.write_facts_batched(&catalog, &budget, &context())?;
    assert!(first.finalized);
    assert!(first.total_batches > 1);
    assert_eq!(
        store.write_facts_batched(&catalog, &budget, &context())?,
        first
    );
    assert_eq!(store.read_facts(&context())?, catalog);
    store.commit_build(&spec, &context())?;
    store.checkpoint()?;
    drop(store);
    assert_eq!(
        GraphDbStore::open(&path, true, GraphLimits::default())?.read_facts(&context())?,
        catalog
    );
    Ok(())
}

#[test]
fn registered_identities_survive_reopen_and_a_second_candidate() -> TestResult {
    let (mut catalog, _) = semantic_fixture()?;
    let input: EntityInput =
        serde_json::from_value(json!({"space_id":"workspace","kind":"team","name":"平台组"}))?;
    let IdentityResolution::Unique(assigned) =
        catalog
            .semantic
            .register_entity(&input, &source("a.py"), 7)?
    else {
        return Err("unique identity".into());
    };
    catalog.semantic.normalize()?;
    let directory = tempfile::tempdir()?;
    let first_path = directory.path().join("first");
    let store = GraphDbStore::create(&first_path, GraphLimits::default())?;
    let first_counts = store.write_facts(&catalog, &context())?;
    store.checkpoint()?;
    drop(store);
    let store = GraphDbStore::open(&first_path, true, GraphLimits::default())?;
    let mut rebuilt = store.read_facts(&context())?;
    drop(store);
    std::fs::remove_dir_all(&first_path)?;
    let origin = SourceRef {
        source_id: "different-source".into(),
        source_version: "v2".into(),
    };
    assert_eq!(
        rebuilt.semantic.register_entity(&input, &origin, 99)?,
        IdentityResolution::Unique(assigned.clone())
    );
    let mut explicit = input.clone();
    explicit.id = Some(assigned.id.clone());
    explicit.name = "新平台组".into();
    assert_eq!(
        rebuilt.semantic.register_entity(&explicit, &origin, 0)?,
        IdentityResolution::Unique(assigned.clone())
    );
    rebuilt
        .semantic
        .entities
        .iter_mut()
        .find(|e| e.identity == assigned)
        .ok_or("entity")?
        .name = explicit.name;
    for index in 0..2 {
        let mut entity = rebuilt.semantic.entities[0].clone();
        entity.identity = id(&format!("new-{index}"));
        entity.kind = format!("new-kind-{index}");
        entity.name = format!("new-name-{index}");
        rebuilt.semantic.entities.push(entity);
        let mut fact = rebuilt.semantic.facts[0].clone();
        fact.identity = id(&format!("new-fact-{index}"));
        fact.predicate = format!("new-predicate-{index}");
        rebuilt.semantic.facts.push(fact);
    }
    rebuilt.semantic.normalize()?;
    let second_path = directory.path().join("second");
    let second = GraphDbStore::create(&second_path, GraphLimits::default())?;
    let second_counts = second.write_facts(&rebuilt, &context())?;
    assert_eq!(
        first_counts.nodes.keys().collect::<Vec<_>>(),
        second_counts.nodes.keys().collect::<Vec<_>>()
    );
    assert_eq!(
        first_counts.relations.keys().collect::<Vec<_>>(),
        second_counts.relations.keys().collect::<Vec<_>>()
    );
    second.checkpoint()?;
    drop(second);
    let result =
        GraphDbStore::open(&second_path, true, GraphLimits::default())?.read_facts(&context())?;
    assert_eq!(result, rebuilt);
    assert!(
        result
            .semantic
            .entities
            .iter()
            .any(|e| e.identity == assigned && e.name == "新平台组")
    );
    Ok(())
}

#[test]
fn semantic_validation_rejects_invalid_endpoints_versions_and_positions_before_writes() -> TestResult
{
    let (catalog, _) = semantic_fixture()?;
    let mut invalid = Vec::new();
    let mut case = catalog.clone();
    case.semantic.facts[0].subject = id("missing");
    invalid.push(case);
    let mut case = catalog.clone();
    case.semantic
        .entities
        .push(case.semantic.entities[0].clone());
    invalid.push(case);
    let mut case = catalog.clone();
    case.semantic.associations[0].evidence.source.source_version = "stale".into();
    invalid.push(case);
    let mut case = catalog.clone();
    case.semantic.mentions[0].evidence.span.end = 1000;
    invalid.push(case);
    let mut case = catalog.clone();
    case.semantic.sources[0].semantic_version = None;
    invalid.push(case);
    let mut case = catalog.clone();
    case.semantic.episodes[0].ingested_at_ms = u64::MAX;
    invalid.push(case);
    let directory = tempfile::tempdir()?;
    let store = GraphDbStore::create(directory.path().join("graph"), GraphLimits::default())?;
    for case in invalid {
        assert!(store.write_facts(&case, &context()).is_err());
        assert!(store.counts()?.nodes.values().all(|count| *count == 0));
        assert!(store.counts()?.relations.values().all(|count| *count == 0));
    }
    Ok(())
}

#[test]
fn semantic_tampering_and_old_format_markers_are_rejected() -> TestResult {
    let (catalog, _) = semantic_fixture()?;
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("graph");
    let store = GraphDbStore::create(&path, GraphLimits::default())?;
    store.write_facts(&catalog, &context())?;
    store.with_write(|tx, options| {
        tx.execute_with(
            "MATCH (e:KgSemanticEntity) WHERE e.entity_id=$id SET e.attributes_json=$value",
            ::graphdb::params! {"id"=>"project","value"=>"[]"},
            options,
        )?;
        Ok(())
    })?;
    assert!(store.read_facts(&context()).is_err());
    drop(store);
    for old_version in [4_u64, 5] {
        let fresh_path = directory.path().join(format!("old-v{old_version}"));
        let fresh = GraphDbStore::create(&fresh_path, GraphLimits::default())?;
        fresh.write_facts(&catalog, &context())?;
        fresh.with_write(|tx, options| {
            tx.execute_with(
                "MATCH (m:KgFactMetadata) SET m.schema_version=$version",
                ::graphdb::params! {"version"=>old_version},
                options,
            )?;
            Ok(())
        })?;
        fresh.checkpoint()?;
        drop(fresh);
        assert_eq!(
            GraphDbStore::open(&fresh_path, true, GraphLimits::default())
                .err()
                .ok_or("old format accepted")?
                .kind,
            GraphErrorKind::IncompatibleFormat
        );
    }
    let rebuilt = GraphDbStore::create(directory.path().join("rebuilt"), GraphLimits::default())?;
    rebuilt.write_facts(&catalog, &context())?;
    assert_eq!(rebuilt.read_facts(&context())?, catalog);
    Ok(())
}

#[test]
fn published_semantic_integrity_rejects_changed_missing_extra_and_duplicate_records() -> TestResult
{
    let (catalog, spec) = semantic_fixture()?;
    for mutation in [
        "MATCH(n:KgSemanticSource) SET n.state='failed'",
        "MATCH(n:KgSemanticEntity) SET n.name='tampered'",
        "MATCH(n:KgEpisode) SET n.content_ref='tampered'",
        "MATCH(n:KgScalarFact) SET n.object_json='false'",
        "MATCH(n:KgSourceAssociation) SET n.role='correction'",
        "MATCH()-[e:KgSemanticRelation]->() SET e.predicate='tampered'",
        "MATCH()-[e:KgMentions]->() SET e.ref_id='tampered'",
        "MATCH(n:KgSourceAssociation) DELETE n",
        "CREATE(:KgSemanticEntity {id:'extra'})",
        "MATCH(a)-[e:KgSemanticRelation]->(b) CREATE(a)-[:KgSemanticRelation {id:e.id}]->(b)",
    ] {
        let dir = tempfile::tempdir()?;
        let store = GraphDbStore::create_build(dir.path().join("graph"), GraphLimits::default())?;
        store.write_facts(&catalog, &context())?;
        store.commit_build(&spec, &context())?;
        assert!(store.inspect_published(&spec, &context())?.is_some());
        store.with_write(|tx, options| {
            tx.execute_with(mutation, (), options)?;
            Ok(())
        })?;
        assert!(store.read_facts(&context()).is_err(), "{mutation}");
        assert!(
            !matches!(store.inspect_published(&spec, &context()), Ok(Some(_))),
            "{mutation}"
        );
    }
    Ok(())
}

#[test]
fn long_scoped_business_and_source_identities_survive_checkpoint() -> TestResult {
    let identity = Identity {
        space_id: "界".repeat(300),
        id: "id".repeat(200),
    };
    let source_id = format!("source:{}", "文".repeat(400));
    let semantic = SemanticCatalog {
        sources: vec![current(&source_id)],
        entities: vec![SemanticEntity {
            identity,
            kind: "person".into(),
            name: "长业务标识".into(),
            aliases: vec![],
            attributes: serde_json::Map::new(),
        }],
        ..Default::default()
    };
    let catalog = FactCatalog::resolve(
        vec![],
        crate::processing::stable_digest(&"config")?,
        crate::processing::stable_digest(&"selection")?,
        &context(),
    )?
    .with_semantic(semantic, &context())?;
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("graph");
    let store = GraphDbStore::create(&path, GraphLimits::default())?;
    store.write_facts(&catalog, &context())?;
    store.checkpoint()?;
    drop(store);
    assert_eq!(
        GraphDbStore::open(&path, true, GraphLimits::default())?.read_facts(&context())?,
        catalog
    );
    Ok(())
}

#[test]
fn declared_fact_states_predecessors_and_correction_evidence_survive_reopen() -> TestResult {
    let (mut catalog, _) = semantic_fixture()?;
    let previous = catalog
        .semantic
        .facts
        .iter_mut()
        .find(|fact| fact.identity == id("f1"))
        .ok_or("entity fact")?;
    previous.state = FactState::Superseded;
    let mut replacement = previous.clone();
    replacement.identity = id("f2");
    replacement.supersedes = Some(id("f1"));
    replacement.changed_at_ms = 200;
    let mut withdrawn = replacement.clone();
    withdrawn.identity = id("f3");
    withdrawn.state = FactState::Retracted;
    withdrawn.supersedes = Some(id("f2"));
    withdrawn.object = FactObject::Scalar {
        value: ScalarValue::Boolean(false),
    };
    withdrawn.changed_at_ms = 300;
    catalog.semantic.facts.extend([replacement, withdrawn]);
    catalog.semantic.associations.push(SourceAssociation {
        target: ObjectId::Fact { identity: id("f2") },
        evidence: SourceEvidence {
            source: SourceRef {
                source_id: "a.py".into(),
                source_version: "v0".into(),
            },
            locator: None,
        },
        role: SourceRole::Correction,
    });
    catalog.semantic.normalize()?;
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("graph");
    let store = GraphDbStore::create(&path, GraphLimits::default())?;
    store.write_facts(&catalog, &context())?;
    assert_eq!(
        store.verify_rows(&context())?,
        codec::FactRows::encode(&catalog)?.digest()?
    );
    store.checkpoint()?;
    drop(store);
    assert_eq!(
        GraphDbStore::open(&path, true, GraphLimits::default())?.read_facts(&context())?,
        catalog
    );
    Ok(())
}

#[test]
fn source_states_and_nullable_withdrawn_episode_round_trip() -> TestResult {
    let states = [
        ("complete", SemanticSourceState::Complete, Some("v1")),
        ("partial", SemanticSourceState::Partial, Some("v0")),
        ("failed", SemanticSourceState::Failed, None),
        ("unspecified", SemanticSourceState::Unspecified, None),
        ("deleted", SemanticSourceState::Deleted, None),
    ];
    let semantic = SemanticCatalog {
        sources: states
            .into_iter()
            .map(|(name, state, version)| SemanticSource {
                source: source(name),
                state,
                semantic_version: version.map(str::to_owned),
            })
            .collect(),
        episodes: vec![Episode {
            identity: id("withdrawn-event"),
            kind: "message".into(),
            source: source("complete"),
            context: "project".into(),
            occurred_at_ms: None,
            ingested_at_ms: 0,
            content_ref: "host:record-2".into(),
            state: EpisodeState::Withdrawn,
        }],
        ..Default::default()
    };
    let catalog = FactCatalog::resolve(
        vec![],
        crate::processing::stable_digest(&"config")?,
        crate::processing::stable_digest(&"selection")?,
        &context(),
    )?
    .with_semantic(semantic, &context())?;
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("graph");
    let store = GraphDbStore::create(&path, GraphLimits::default())?;
    store.write_facts(&catalog, &context())?;
    store.checkpoint()?;
    drop(store);
    assert_eq!(
        GraphDbStore::open(&path, true, GraphLimits::default())?.read_facts(&context())?,
        catalog
    );
    Ok(())
}

#[test]
fn reopened_registry_keeps_ambiguity_conflicts_and_source_symbols_separate() -> TestResult {
    let (mut catalog, _) = semantic_fixture()?;
    let duplicate: EntityInput = serde_json::from_value(
        json!({"space_id":"workspace","id":"second-person","kind":"person","name":"张三"}),
    )?;
    assert_eq!(
        catalog
            .semantic
            .register_entity(&duplicate, &source("a.py"), 8)?,
        IdentityResolution::Unique(id("second-person"))
    );
    let business: EntityInput = serde_json::from_value(
        json!({"space_id":"workspace","id":"business-helper","kind":"function","name":"helper"}),
    )?;
    assert_eq!(
        catalog
            .semantic
            .register_entity(&business, &source("a.py"), 9)?,
        IdentityResolution::Unique(id("business-helper"))
    );
    let other_kind: EntityInput = serde_json::from_value(
        json!({"space_id":"workspace","id":"named-team","kind":"team","name":"张三"}),
    )?;
    assert_eq!(
        catalog
            .semantic
            .register_entity(&other_kind, &source("a.py"), 10)?,
        IdentityResolution::Unique(id("named-team"))
    );
    catalog.semantic.normalize()?;
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("graph");
    let store = GraphDbStore::create(&path, GraphLimits::default())?;
    store.write_facts(&catalog, &context())?;
    store.checkpoint()?;
    drop(store);
    let mut reopened =
        GraphDbStore::open(&path, true, GraphLimits::default())?.read_facts(&context())?;
    let by_name: EntityInput =
        serde_json::from_value(json!({"space_id":"workspace","kind":"person","name":"张三"}))?;
    assert_eq!(
        reopened
            .semantic
            .register_entity(&by_name, &source("a.py"), 0)?,
        IdentityResolution::Ambiguous(vec![id("person"), id("second-person")])
    );
    let conflict: EntityInput = serde_json::from_value(
        json!({"space_id":"workspace","id":"person","kind":"person","name":"张三","attributes":{"changed":true}}),
    )?;
    assert_eq!(
        reopened
            .semantic
            .register_entity(&conflict, &source("a.py"), 0)?,
        IdentityResolution::Conflict(id("person"))
    );
    assert_eq!(reopened.semantic, catalog.semantic);
    let mut team_by_name = other_kind;
    team_by_name.id = None;
    assert_eq!(
        reopened
            .semantic
            .register_entity(&team_by_name, &source("a.py"), 0)?,
        IdentityResolution::Unique(id("named-team"))
    );
    let symbol = reopened
        .files
        .iter()
        .filter_map(|file| file.facts.as_ref())
        .flat_map(|file| &file.nodes)
        .find(|node| node.name == "helper")
        .ok_or("source symbol")?;
    assert_ne!(symbol.id, id("business-helper").storage_key()?);
    Ok(())
}
