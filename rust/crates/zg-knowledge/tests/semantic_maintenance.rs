use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet};
use zg_knowledge::{
    Error,
    processing::{ProcessingContext, stable_digest},
    relations::bound::FactCatalog,
    semantic::*,
};

#[derive(Deserialize)]
struct Inputs {
    cases: Vec<Case>,
}
#[derive(Deserialize)]
struct Case {
    name: String,
    input: SemanticInput,
}
#[derive(Deserialize)]
struct Expectations {
    version: u32,
    input_fixture_sha256: String,
    scenarios: Vec<Scenario>,
}
#[derive(Deserialize)]
struct Scenario {
    name: String,
    steps: Vec<Step>,
}
#[derive(Deserialize)]
struct Step {
    case: String,
    current: Vec<String>,
    support: BTreeMap<String, Vec<String>>,
    withdrawn: BTreeMap<String, Vec<String>>,
    source_state: Option<SemanticSource>,
}
fn context() -> ProcessingContext {
    ProcessingContext {
        max_input_bytes: 4 << 20,
        max_output_items: 100_000,
        deadline: None,
        cancellation: tokio_util::sync::CancellationToken::new(),
    }
}
fn input(name: &str) -> SemanticInput {
    let values: Inputs =
        serde_json::from_str(include_str!("fixtures/semantic-source-changes-v1.json"))
            .expect("fixed inputs");
    values
        .cases
        .into_iter()
        .find(|c| c.name == name)
        .expect("case")
        .input
}
fn scenarios() -> Vec<Scenario> {
    let values: Expectations =
        serde_json::from_str(include_str!("fixtures/semantic-source-maintenance-v1.json"))
            .expect("fixed expectations");
    assert_eq!(values.version, 1);
    assert_eq!(
        values.input_fixture_sha256,
        zg_storage::sha256_hex(include_bytes!("fixtures/semantic-source-changes-v1.json"))
    );
    values.scenarios
}
fn empty() -> FactCatalog {
    FactCatalog::resolve(
        vec![],
        stable_digest(&serde_json::json!({})).expect("policy"),
        stable_digest(&"selection").expect("selection"),
        &context(),
    )
    .expect("empty catalog")
}
fn origin(role: SourceRole, data: &SemanticCatalog) -> BTreeMap<String, Vec<String>> {
    let mut rows: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for a in data.associations.iter().filter(|a| a.role == role) {
        if let ObjectId::Fact { identity } = &a.target {
            rows.entry(identity.id.clone())
                .or_default()
                .insert(a.evidence.source.source_id.clone());
        }
    }
    rows.into_iter()
        .map(|(id, sources)| (id, sources.into_iter().collect()))
        .collect()
}
fn check(data: &SemanticCatalog, step: &Step) {
    assert_eq!(
        data.current_facts()
            .map(|f| f.identity.id.clone())
            .collect::<Vec<_>>(),
        step.current,
        "{}: current",
        step.case
    );
    assert_eq!(
        origin(SourceRole::Support, data),
        step.support,
        "{}: support",
        step.case
    );
    assert_eq!(
        origin(SourceRole::Withdrawn, data),
        step.withdrawn,
        "{}: withdrawn",
        step.case
    );
    assert_eq!(data.entities.len(), 3, "identities retained");
    assert!(
        data.facts
            .iter()
            .all(|f| f.state == FactState::Active && f.supersedes.is_none()),
        "source changes are not business retractions"
    );
    if let Some(expected) = &step.source_state {
        assert!(
            data.sources.contains(expected),
            "{}: source state",
            step.case
        );
    }
}
#[test]
fn frozen_current_source_cases_preserve_contributions_and_identity() {
    for scenario in scenarios() {
        let mut catalog = empty();
        for step in scenario.steps {
            catalog = catalog
                .apply_sources(&input(&step.case), &context())
                .expect(&scenario.name);
            check(&catalog.semantic, &step);
        }
    }
}
#[test]
fn source_conflicts_and_incomplete_results_never_mutate_the_base() {
    let base = empty()
        .apply_sources(&input("initial"), &context())
        .expect("initial");
    let frozen = base.clone();
    let mut changed = input("initial");
    let SourceChange::Upsert {
        semantic: Some(SemanticUpdate::Complete(batch)),
        ..
    } = &mut changed.sources[0]
    else {
        panic!("complete fixture")
    };
    batch.facts[0]
        .attributes
        .insert("different".into(), serde_json::json!(true));
    assert!(matches!(
        base.clone().apply_sources(&changed, &context()),
        Err(Error::Conflict(_))
    ));
    for name in ["semantic-partial", "semantic-failed", "semantic-omitted"] {
        let mut change = input(name);
        if let SourceChange::Upsert {
            semantic: Some(SemanticUpdate::Partial(batch)),
            ..
        } = &mut change.sources[0]
        {
            batch.entities = input("initial")
                .sources
                .into_iter()
                .find_map(|c| match c {
                    SourceChange::Upsert {
                        semantic: Some(SemanticUpdate::Complete(b)),
                        ..
                    } => Some(b.entities),
                    _ => None,
                })
                .expect("entities");
            batch.entities[0].name = "unfinished".into();
        }
        let next = base
            .clone()
            .apply_sources(&change, &context())
            .expect("incomplete");
        assert_eq!(next.semantic.entities, base.semantic.entities);
        assert_eq!(next.semantic.facts, base.semantic.facts);
        assert_eq!(next.semantic.associations, base.semantic.associations);
    }
    let mut deleted = input("delete-a");
    if let SourceChange::Delete { source } = &mut deleted.sources[0] {
        source.source_version = "stale".into();
    }
    assert!(matches!(
        base.clone().apply_sources(&deleted, &context()),
        Err(Error::Conflict(_))
    ));
    assert_eq!(base, frozen);
}
#[test]
fn boundaries_and_budgets_fail_explicitly() {
    let base = empty()
        .apply_sources(&input("initial"), &context())
        .expect("initial");
    for (name, state) in [
        ("explicit-correction", FactState::Superseded),
        ("explicit-retraction", FactState::Retracted),
    ] {
        let updated = base
            .clone()
            .apply_sources(&input(name), &context())
            .expect("explicit change");
        assert_eq!(updated.semantic.facts[0].state, state);
        assert_eq!(updated.semantic.changes.len(), 1);
    }
    let mut limited = context();
    limited.max_input_bytes = 1;
    assert!(matches!(
        base.clone().apply_sources(&input("initial"), &limited),
        Err(Error::ResourceExhausted)
    ));
    let mut limited = context();
    limited.max_output_items = 0;
    assert!(matches!(
        base.clone().apply_sources(&input("initial"), &limited),
        Err(Error::ResourceExhausted)
    ));
    let cancelled = context();
    cancelled.cancellation.cancel();
    assert!(matches!(
        base.clone().apply_sources(&input("initial"), &cancelled),
        Err(Error::Cancelled)
    ));
    let mut expired = context();
    expired.deadline = Some(std::time::Instant::now());
    assert!(matches!(
        base.clone().apply_sources(&input("initial"), &expired),
        Err(Error::Deadline)
    ));
    let mut outside = input("initial");
    outside.update_scope.clear();
    assert!(matches!(
        base.clone().apply_sources(&outside, &context()),
        Err(Error::InvalidInput(_))
    ));
    assert_eq!(base.semantic.current_facts().count(), 1);
}
#[test]
fn omitted_attributes_and_replayed_source_assertions_preserve_existing_states() {
    let mut base = empty()
        .apply_sources(&input("initial"), &context())
        .expect("initial");
    base.semantic.entities[0]
        .attributes
        .insert("preserved".into(), serde_json::json!([true, 3]));
    base.semantic.facts[0].state = FactState::Retracted;
    let next = base
        .clone()
        .apply_sources(&input("initial"), &context())
        .expect("replayed assertion");
    assert_eq!(next.semantic.entities, base.semantic.entities);
    assert_eq!(next.semantic.facts, base.semantic.facts);
    assert_eq!(next.semantic.current_facts().count(), 0);
}

#[cfg(feature = "graphdb")]
#[test]
fn frozen_source_sequences_survive_real_candidate_commits_and_reopen()
-> Result<(), Box<dyn std::error::Error>> {
    use zg_knowledge::graph::{GraphDbStore, GraphLimits, ImportBudget};
    let dir = tempfile::tempdir()?;
    let mut ordinal = 0;
    for scenario in scenarios() {
        let mut catalog = empty();
        let mut previous = None;
        for step in scenario.steps {
            ordinal += 1;
            catalog = catalog.apply_sources(&input(&step.case), &context())?;
            let path = dir.path().join(format!("candidate-{ordinal}"));
            let spec = build_spec(&catalog, ordinal)?;
            let store = GraphDbStore::create_batched_build(&path, GraphLimits::default())?;
            store.write_facts_batched(
                &catalog,
                &ImportBudget {
                    max_batch_rows: 3,
                    max_batch_bytes: 4096,
                },
                &context(),
            )?;
            let counts = store.counts()?;
            let proof = store.commit_build(&spec, &context())?;
            store.checkpoint()?;
            drop(store);
            if let Some(old) = previous.take() {
                std::fs::remove_dir_all(old)?;
            }
            let read = GraphDbStore::open(&path, true, GraphLimits::default())?;
            assert_eq!(read.counts()?, counts);
            assert_eq!(read.inspect_build(&spec, &context())?, Some(proof));
            assert_eq!(read.read_facts(&context())?, catalog);
            catalog = read.read_facts(&context())?;
            check(&catalog.semantic, &step);
            drop(read);
            previous = Some(path);
        }
        if let Some(old) = previous {
            std::fs::remove_dir_all(old)?;
        }
    }
    Ok(())
}

fn complete_batch(input: &mut SemanticInput) -> &mut SemanticBatch {
    let SourceChange::Upsert {
        semantic: Some(SemanticUpdate::Complete(batch)),
        ..
    } = &mut input.sources[0]
    else {
        panic!("complete input")
    };
    batch
}
#[test]
fn source_replacement_preserves_other_spaces_episodes_and_supporters() {
    let base = empty()
        .apply_sources(&input("initial"), &context())
        .expect("initial")
        .apply_sources(&input("second-source"), &context())
        .expect("second");
    let outside:SemanticInput=serde_json::from_value(serde_json::json!({
        "space_id":"other","update_scope":["outside"],
        "sources":[{"change":"upsert","source":{"source_id":"outside","source_version":"o1"},"semantic":{"status":"complete","value":{"entities":[{"space_id":"other","id":"zhang","kind":"person","name":"张三","attributes":{"keep":true}}],"facts":[],"mentions":[],"associations":[]}}}],
        "episodes":[{"identity":{"space_id":"other","id":"event"},"kind":"meeting","source":{"source_id":"outside","source_version":"o1"},"context":"outside","occurred_at_ms":null,"ingested_at_ms":100,"content_ref":"host:outside","state":"active"}],"fact_changes":[]
    })).expect("outside input");
    let base = base.apply_sources(&outside, &context()).expect("outside");
    let mut replacement = input("initial");
    if let SourceChange::Upsert { source, .. } = &mut replacement.sources[0] {
        source.source_version = "v2".into();
    }
    let batch = complete_batch(&mut replacement);
    batch.entities.clear();
    batch.facts[0].identity.id = "f3".into();
    batch.facts[0].object = FactObject::Entity {
        identity: Identity {
            space_id: "workspace".into(),
            id: "li".into(),
        },
    };
    let next = base
        .clone()
        .apply_sources(&replacement, &context())
        .expect("replacement");
    assert_eq!(
        next.semantic
            .current_facts()
            .map(|f| f.identity.id.as_str())
            .collect::<Vec<_>>(),
        vec!["f1", "f3"]
    );
    assert_eq!(
        origin(SourceRole::Support, &next.semantic)["f1"],
        vec!["b.md"]
    );
    assert_eq!(
        origin(SourceRole::Support, &next.semantic)["f3"],
        vec!["a.md"]
    );
    assert_eq!(next.semantic.entities, base.semantic.entities);
    assert_eq!(next.semantic.episodes, base.semantic.episodes);
    assert!(
        next.semantic.sources.contains(
            base.semantic
                .sources
                .iter()
                .find(|s| s.source.source_id == "outside")
                .expect("outside source")
        )
    );
}
#[test]
fn fact_identity_checks_full_assertion_and_preserves_the_first_timestamp() {
    let base = empty()
        .apply_sources(&input("initial"), &context())
        .expect("initial");
    let mut second = input("second-source");
    complete_batch(&mut second).facts[0].changed_at_ms = 900;
    let next = base
        .clone()
        .apply_sources(&second, &context())
        .expect("same fact another observation");
    assert_eq!(next.semantic.facts, base.semantic.facts);
    for field in ["context", "attributes", "object", "predicate"] {
        let mut invalid = second.clone();
        let fact = &mut complete_batch(&mut invalid).facts[0];
        match field {
            "context" => fact.context = "other".into(),
            "attributes" => {
                fact.attributes
                    .insert("different".into(), serde_json::json!(true));
            }
            "object" => {
                fact.object = FactObject::Scalar {
                    value: ScalarValue::Boolean(true),
                }
            }
            _ => fact.predicate = "different".into(),
        }
        assert!(
            matches!(
                base.clone().apply_sources(&invalid, &context()),
                Err(Error::Conflict(_))
            ),
            "{field}"
        );
    }
    let mut distinct = second;
    complete_batch(&mut distinct).facts[0].identity.id = "explicitly-distinct".into();
    assert_eq!(
        base.apply_sources(&distinct, &context())
            .expect("explicit identity")
            .semantic
            .current_facts()
            .count(),
        2,
        "equal triples do not override distinct explicit IDs"
    );
}
#[test]
fn generated_entity_id_is_reused_after_replacement_and_ambiguous_input_fails() {
    let mut initial = input("initial");
    let batch = complete_batch(&mut initial);
    batch.facts.clear();
    batch.entities.truncate(1);
    batch.entities[0].id = None;
    let base = empty()
        .apply_sources(&initial, &context())
        .expect("generated identity");
    let mut later = initial.clone();
    if let SourceChange::Upsert { source, .. } = &mut later.sources[0] {
        source.source_version = "v2".into();
    }
    let next = base
        .clone()
        .apply_sources(&later, &context())
        .expect("source replacement");
    assert_eq!(next.semantic.entities, base.semantic.entities);
    let mut duplicate = initial.clone();
    complete_batch(&mut duplicate).entities[0].id = Some("explicit-second".into());
    let duplicate = base
        .apply_sources(&duplicate, &context())
        .expect("explicit same name");
    let entity = &complete_batch(&mut later).entities[0];
    assert!(matches!(
        resolve_identity(entity, &duplicate.semantic.entities).expect("lookup"),
        IdentityResolution::Ambiguous(_)
    ));
    assert!(matches!(
        duplicate.apply_sources(&later, &context()),
        Err(Error::Conflict(_))
    ));
}
#[test]
fn only_source_support_is_accepted_from_batches_and_correction_records_are_inherited() {
    let mut base = empty()
        .apply_sources(&input("initial"), &context())
        .expect("initial");
    let mut history = base
        .semantic
        .associations
        .iter()
        .find(|a| matches!(a.target, ObjectId::Fact { .. }))
        .expect("association")
        .clone();
    history.role = SourceRole::Correction;
    history.evidence.source.source_version = "historical".into();
    base.semantic.associations.push(history.clone());
    base.semantic.normalize().expect("canonical");
    let removed = base
        .clone()
        .apply_sources(&input("delete-a"), &context())
        .expect("removed");
    let mut withdrawn = history.clone();
    withdrawn.role = SourceRole::Withdrawn;
    assert!(removed.semantic.associations.contains(&withdrawn));
    for role in [SourceRole::Correction, SourceRole::Withdrawn] {
        let mut invalid = input("initial");
        let batch = complete_batch(&mut invalid);
        let mut association = history.clone();
        association.evidence.source.source_version = "v1".into();
        association.role = role;
        batch.associations.push(association);
        assert!(matches!(
            base.clone().apply_sources(&invalid, &context()),
            Err(Error::InvalidInput(_))
        ));
    }
    let mut invalid = input("initial");
    let batch = complete_batch(&mut invalid);
    let mut support = history;
    support.role = SourceRole::Support;
    support.evidence.source.source_version = "v1".into();
    batch.associations.extend([support.clone(), support]);
    assert!(
        matches!(
            base.apply_sources(&invalid, &context()),
            Err(Error::InvalidInput(_))
        ),
        "duplicate source associations are not silently deduplicated"
    );
}

#[cfg(feature = "graphdb")]
fn build_spec(
    catalog: &FactCatalog,
    ordinal: u64,
) -> zg_knowledge::Result<zg_knowledge::graph::build::BuildSpec> {
    use zg_knowledge::graph::build::{BuildScope, BuildSpec};
    Ok(BuildSpec {
        graph_build_id: format!("00000000-0000-4000-8000-{ordinal:012x}"),
        operation_id: format!("10000000-0000-4000-8000-{ordinal:012x}"),
        attempt: 1,
        base_text_revision: 1,
        base_catalog_digest: stable_digest(&Vec::<String>::new())?,
        base_head: None,
        scope: BuildScope::Both,
        configuration: serde_json::json!({}),
        policy_fingerprint: catalog.config_fingerprint.clone(),
        rules_fingerprint: stable_digest(&"source-maintenance-v1")?,
        module_roots_fingerprint: stable_digest(&"roots")?,
    })
}

#[cfg(feature = "graphdb")]
#[test]
fn the_same_attempt_replays_and_changed_source_input_cannot_overwrite_it()
-> Result<(), Box<dyn std::error::Error>> {
    use zg_knowledge::graph::{GraphDbStore, GraphErrorKind, GraphLimits, ImportBudget};
    let catalog = empty().apply_sources(&input("initial"), &context())?;
    let changed = catalog
        .clone()
        .apply_sources(&input("delete-a"), &context())?;
    let dir = tempfile::tempdir()?;
    let store =
        GraphDbStore::create_batched_build(dir.path().join("graph"), GraphLimits::default())?;
    let budget = ImportBudget {
        max_batch_rows: 3,
        max_batch_bytes: 4096,
    };
    let progress = store.write_facts_batched(&catalog, &budget, &context())?;
    assert_eq!(
        store.write_facts_batched(&catalog, &budget, &context())?,
        progress
    );
    assert_eq!(
        store
            .write_facts_batched(&changed, &budget, &context())
            .expect_err("changed source input")
            .kind,
        GraphErrorKind::Conflict
    );
    assert_eq!(store.read_facts(&context())?, catalog);
    let spec = build_spec(&catalog, 1)?;
    let proof = store.commit_build(&spec, &context())?;
    assert_eq!(store.commit_build(&spec, &context())?, proof);
    Ok(())
}
