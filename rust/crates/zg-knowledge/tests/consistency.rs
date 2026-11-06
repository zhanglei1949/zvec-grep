use zg_knowledge::{processing::ProcessingContext, semantic::*};

fn context() -> ProcessingContext {
    ProcessingContext {
        max_input_bytes: 4 << 20,
        max_output_items: 100_000,
        deadline: None,
        cancellation: tokio_util::sync::CancellationToken::new(),
    }
}
fn input(name: &str) -> SemanticInput {
    let data: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/semantic-source-changes-v1.json"))
            .expect("valid fixture/result");
    serde_json::from_value(
        data["cases"]
            .as_array()
            .expect("valid fixture/result")
            .iter()
            .find(|c| c["name"] == name)
            .expect("valid fixture/result")["input"]
            .clone(),
    )
    .expect("valid fixture/result")
}
#[test]
fn explicit_supports_and_system_times_survive_correction_replay() {
    let base = SemanticCatalog::default()
        .apply_sources_at(&input("initial"), &context(), 1000)
        .expect("valid fixture/result");
    let corrected = base
        .apply_sources_at(&input("explicit-correction"), &context(), 2000)
        .expect("valid fixture/result");
    let fact = corrected
        .current_facts()
        .next()
        .expect("replacement is supported");
    assert_eq!(fact.created_at_ms, Some(2000));
    assert_eq!(fact.changed_at_ms, 2000);
    assert_eq!(
        corrected
            .facts
            .iter()
            .find(|f| f.identity.id == "f1")
            .expect("predecessor")
            .changed_at_ms,
        2000
    );
    assert!(
        corrected
            .associations
            .iter()
            .any(|a| a.role == SourceRole::Support
                && matches!(&a.target,ObjectId::Fact{identity} if identity==&fact.identity))
    );
    let replayed = corrected
        .apply_sources_at(&input("explicit-correction"), &context(), 9000)
        .expect("valid fixture/result");
    assert_eq!(
        replayed, corrected,
        "replay must not refresh times or retain duplicate transient withdrawal markers"
    );
    assert_eq!(
        corrected
            .facts
            .iter()
            .find(|f| f.identity.id == "f1")
            .expect("valid fixture/result")
            .created_at_ms,
        Some(1000)
    );
    let mut without_support = corrected.clone();
    without_support
        .associations
        .retain(|a| a.role != SourceRole::Support);
    assert_eq!(
        without_support.current_facts().count(),
        0,
        "correction evidence alone never supports facts"
    );
}
#[test]
fn fact_system_times_ignore_input_clocks_and_only_change_on_business_updates() {
    let mut initial = input("initial");
    let SourceChange::Upsert {
        semantic: Some(SemanticUpdate::Complete(batch)),
        ..
    } = &mut initial.sources[0]
    else {
        panic!("complete fixture");
    };
    batch.facts[0].changed_at_ms = u64::MAX;
    let base = SemanticCatalog::default()
        .apply_sources_at(&initial, &context(), 1000)
        .expect("initial");
    assert_eq!(base.facts[0].changed_at_ms, 1000);
    let supported = base
        .apply_sources_at(&input("second-source"), &context(), 2000)
        .expect("another support");
    assert_eq!(supported.facts, base.facts);
    let retracted = supported
        .apply_sources_at(&input("explicit-retraction"), &context(), 3000)
        .expect("retract");
    assert_eq!(retracted.facts[0].created_at_ms, Some(1000));
    assert_eq!(retracted.facts[0].changed_at_ms, 3000);
    assert_eq!(retracted.facts[0].state, FactState::Retracted);
    assert_eq!(
        retracted
            .apply_sources_at(&input("explicit-retraction"), &context(), 9000)
            .expect("replay"),
        retracted
    );
}
#[test]
fn episode_withdrawal_only_removes_its_own_support() {
    let mut initial = input("initial");
    let episode = Episode {
        identity: Identity {
            space_id: "workspace".into(),
            id: "e1".into(),
        },
        kind: "message".into(),
        source: initial.sources[0].source().clone(),
        context: "project-a".into(),
        occurred_at_ms: None,
        ingested_at_ms: 10,
        content_ref: "body/e1".into(),
        state: EpisodeState::Active,
    };
    initial.episodes.push(episode.clone());
    let base = SemanticCatalog::default()
        .apply_sources_at(&initial, &context(), 1000)
        .expect("valid fixture/result");
    assert_eq!(base.current_facts().count(), 1);
    let mut withdraw = input("scope-without-changes");
    withdraw.episode_withdrawals.push(EpisodeWithdrawal {
        target: episode.identity.clone(),
        expected_state: EpisodeState::Active,
        context: episode.context.clone(),
        changed_at_ms: 2000,
        evidence: SourceEvidence {
            source: episode.source.clone(),
            episode: Some(episode.identity.clone()),
            locator: None,
        },
    });
    assert_eq!(
        base.apply_sources_at(&withdraw, &context(), 2000)
            .expect("valid fixture/result")
            .current_facts()
            .count(),
        0
    );
    let both = base
        .apply_sources_at(&input("second-source"), &context(), 1500)
        .expect("valid fixture/result");
    let withdrawn = both
        .apply_sources_at(&withdraw, &context(), 2000)
        .expect("valid fixture/result");
    assert_eq!(
        withdrawn.current_facts().count(),
        1,
        "an independent source still supports the fact"
    );
    assert_eq!(
        withdrawn
            .apply_sources_at(&withdraw, &context(), 3000)
            .expect("valid fixture/result"),
        withdrawn
    );
}
#[test]
fn source_version_changes_hide_stale_facts_but_keep_history() {
    let base = SemanticCatalog::default()
        .apply_sources_at(&input("initial"), &context(), 1000)
        .expect("valid fixture/result");
    let changed = base
        .apply_sources_at(&input("semantic-omitted"), &context(), 2000)
        .expect("valid fixture/result");
    assert_eq!(changed.current_facts().count(), 0);
    assert_eq!(changed.facts, base.facts);
    assert_eq!(changed.associations, base.associations);
}
#[test]
fn new_input_cannot_supply_a_creation_time() {
    let mut initial = input("initial");
    let SourceChange::Upsert {
        semantic: Some(SemanticUpdate::Complete(batch)),
        ..
    } = &mut initial.sources[0]
    else {
        panic!("complete fixture")
    };
    batch.facts[0].created_at_ms = Some(1);
    assert!(
        SemanticCatalog::default()
            .apply_sources_at(&initial, &context(), 2000)
            .is_err()
    );
}
#[test]
fn episode_links_validate_endpoints_and_preserve_explicit_identity() {
    let mut initial = input("initial");
    let source = initial.sources[0].source().clone();
    for id in ["request", "result"] {
        initial.episodes.push(Episode {
            identity: Identity {
                space_id: "workspace".into(),
                id: id.into(),
            },
            kind: "tool".into(),
            source: source.clone(),
            context: "project-a".into(),
            occurred_at_ms: None,
            ingested_at_ms: 10,
            content_ref: id.into(),
            state: EpisodeState::Active,
        });
    }
    // Shared raw sources need explicit evidence ownership; omit semantic contribution here.
    if let SourceChange::Upsert { semantic, .. } = &mut initial.sources[0] {
        *semantic = None;
    }
    initial.episode_links.push(EpisodeLink {
        identity: Identity {
            space_id: "workspace".into(),
            id: "l1".into(),
        },
        source: initial.episodes[1].identity.clone(),
        target: initial.episodes[0].identity.clone(),
        relation_type: EpisodeRelation::ResultOf,
    });
    let base = SemanticCatalog::default()
        .apply_sources_at(&initial, &context(), 1000)
        .expect("valid fixture/result");
    assert_eq!(base.episode_links.len(), 1);
    let mut invalid = initial.clone();
    invalid.episode_links[0].target = invalid.episode_links[0].source.clone();
    assert!(
        SemanticCatalog::default()
            .apply_sources_at(&invalid, &context(), 1000)
            .is_err()
    );
}
