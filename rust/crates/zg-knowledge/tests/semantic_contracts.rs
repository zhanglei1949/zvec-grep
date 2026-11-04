use serde_json::json;
use zg_knowledge::{
    Error,
    query::{ByteSpan, EvidenceRef},
    semantic::*,
};

fn id(value: &str) -> Identity {
    Identity {
        space_id: "workspace".into(),
        id: value.into(),
    }
}
fn source() -> SourceRef {
    SourceRef {
        source_id: "document.md".into(),
        source_version: "revision-1".into(),
    }
}
fn evidence() -> SourceEvidence {
    SourceEvidence {
        source: source(),
        locator: None,
    }
}
fn entity(value: &str) -> SemanticEntity {
    SemanticEntity {
        identity: id(value),
        kind: "person".into(),
        name: "张三".into(),
        aliases: vec!["Zhang".into()],
        attributes: serde_json::Map::new(),
    }
}
fn entity_input() -> EntityInput {
    EntityInput {
        space_id: "workspace".into(),
        id: None,
        kind: "person".into(),
        name: "张三".into(),
        aliases: vec![],
        attributes: None,
    }
}
fn fact(value: &str) -> Fact {
    Fact {
        identity: id(value),
        subject: id("project"),
        predicate: "OWNER".into(),
        object: FactObject::Entity {
            identity: id("person"),
        },
        context: "project".into(),
        state: FactState::Active,
        supersedes: None,
        changed_at_ms: 100,
        attributes: serde_json::Map::new(),
    }
}
fn batch() -> SemanticBatch {
    SemanticBatch {
        entities: vec![],
        facts: vec![],
        mentions: vec![],
        associations: vec![],
    }
}
fn input(change: SourceChange) -> SemanticInput {
    SemanticInput {
        space_id: "workspace".into(),
        update_scope: vec![source().source_id],
        sources: vec![change],
        episodes: vec![],
        fact_changes: vec![],
        episode_withdrawals: vec![],
    }
}

#[test]
fn scoped_identity_and_object_types_do_not_collapse_namespaces() {
    let a = Identity {
        space_id: "a:b".into(),
        id: "c".into(),
    };
    let b = Identity {
        space_id: "a".into(),
        id: "b:c".into(),
    };
    assert_ne!(a.storage_key().expect("key"), b.storage_key().expect("key"));
    let entity = ObjectId::SemanticEntity {
        identity: id("same"),
    };
    let episode = ObjectId::Episode {
        identity: id("same"),
    };
    assert_ne!(
        serde_json::to_value(entity).expect("entity"),
        serde_json::to_value(episode).expect("episode")
    );
    assert!(
        Identity {
            space_id: " ".into(),
            id: "p".into()
        }
        .validate()
        .is_err()
    );
    assert!(
        ObjectId::SourceNode {
            node_id: String::new()
        }
        .validate()
        .is_err()
    );
}

#[test]
fn identity_matching_obeys_explicit_ids_space_kind_ambiguity_and_conflict() {
    let mut other_space = entity("outside");
    other_space.identity.space_id = "other".into();
    let mut other_kind = entity("project");
    other_kind.kind = "project".into();
    let catalog = vec![entity("p1"), entity("p2"), other_space, other_kind];
    let mut request = entity_input();
    assert_eq!(
        resolve_identity(&request, &catalog).expect("matching"),
        IdentityResolution::Ambiguous(vec![id("p1"), id("p2")])
    );
    request.id = Some("p2".into());
    request.name = "renamed".into();
    assert_eq!(
        resolve_identity(&request, &catalog).expect("business ID"),
        IdentityResolution::Unique(id("p2"))
    );
    request.kind = "project".into();
    assert_eq!(
        resolve_identity(&request, &catalog).expect("conflict"),
        IdentityResolution::Conflict(id("p2"))
    );
    request = entity_input();
    request.name = "Zhang".into();
    assert_eq!(
        resolve_identity(&request, &catalog[..1]).expect("alias"),
        IdentityResolution::Unique(id("p1"))
    );
    request.name = "zhang".into();
    assert_eq!(
        resolve_identity(&request, &catalog).expect("case-sensitive"),
        IdentityResolution::New
    );
    request = entity_input();
    request.attributes = Some(serde_json::from_value(json!({"role":"owner"})).expect("object"));
    assert_eq!(
        resolve_identity(&request, &catalog[..1]).expect("attributes conflict"),
        IdentityResolution::Conflict(id("p1"))
    );
}

#[test]
fn attributes_distinguish_omission_empty_objects_and_invalid_values() {
    let schema = serde_json::to_value(schemars::schema_for!(EntityInput)).expect("input schema");
    assert_eq!(schema["properties"]["attributes"]["type"], "object");
    assert!(
        !schema["required"]
            .as_array()
            .expect("required fields")
            .contains(&json!("attributes"))
    );
    let base = json!({"space_id":"workspace","kind":"person","name":"张三"});
    let omitted: EntityInput = serde_json::from_value(base.clone()).expect("omitted");
    assert!(omitted.attributes.is_none());
    let mut provided = base.clone();
    provided["attributes"] = json!({});
    let empty: EntityInput = serde_json::from_value(provided.clone()).expect("empty object");
    assert_eq!(empty.attributes, Some(serde_json::Map::new()));
    provided["attributes"] =
        json!({"allocation":0.5,"active":true,"tags":["a",3],"identity":"business attribute"});
    let parsed: EntityInput = serde_json::from_value(provided.clone()).expect("typed attributes");
    let encoded = serde_json::to_value(parsed).expect("round trip");
    assert_eq!(encoded["attributes"], provided["attributes"]);
    assert_eq!(encoded["space_id"], "workspace");
    for invalid in [json!(null), json!([]), json!(3), json!("{}")] {
        let mut value = base.clone();
        value["attributes"] = invalid;
        assert!(serde_json::from_value::<EntityInput>(value).is_err());
    }
}

#[test]
fn facts_preserve_typed_scalars_and_reject_wrong_spaces_and_self_correction() {
    let mut value = fact("f1");
    value.validate().expect("entity fact");
    if let FactObject::Entity { identity } = &mut value.object {
        identity.space_id = "other".into();
    }
    assert!(value.validate().is_err());
    value = fact("f1");
    value.supersedes = Some(id("f1"));
    assert!(value.validate().is_err());
    for scalar in [json!("张三"), json!(0.5), json!(false), json!(null)] {
        let object: FactObject =
            serde_json::from_value(json!({"type":"scalar","value":scalar})).expect("scalar");
        assert_eq!(
            serde_json::to_value(object).expect("round trip")["value"],
            scalar
        );
    }
    assert!(
        serde_json::from_value::<FactObject>(json!({"type":"scalar","value":{"arbitrary":true}}))
            .is_err()
    );
    let mut encoded = serde_json::to_value(fact("f1")).expect("record");
    encoded
        .as_object_mut()
        .expect("object")
        .remove("attributes");
    assert!(
        serde_json::from_value::<Fact>(encoded).is_err(),
        "complete replacement cannot silently clear omitted attributes"
    );
}

#[test]
fn corrections_check_predecessor_context_and_expected_state_without_reactivation() {
    let current = fact("f1");
    let mut replacement = fact("f2");
    replacement.supersedes = Some(id("f1"));
    let change = FactChange::Supersede {
        target: id("f1"),
        expected_state: FactState::Active,
        replacement: Box::new(replacement),
        evidence: evidence(),
    };
    change.validate().expect("explicit replacement");
    change
        .check_current(&current)
        .expect("matching predecessor");
    for state in [FactState::Superseded, FactState::Retracted] {
        let mut old = current.clone();
        old.state = state;
        assert!(matches!(
            change.check_current(&old),
            Err(Error::Conflict(_))
        ));
    }
    let mut different = current.clone();
    different.context = "other project".into();
    assert!(matches!(
        change.check_current(&different),
        Err(Error::Conflict(_))
    ));
    let invalid = FactChange::Supersede {
        target: id("f1"),
        expected_state: FactState::Active,
        replacement: Box::new(fact("f2")),
        evidence: evidence(),
    };
    assert!(invalid.validate().is_err());
    let invalid = FactChange::Retract {
        target: id("f1"),
        expected_state: FactState::Retracted,
        context: "project".into(),
        changed_at_ms: 101,
        evidence: evidence(),
    };
    assert!(invalid.validate().is_err());
}

#[test]
fn source_changes_keep_missing_partial_failed_empty_and_deleted_distinct() {
    let changes = [
        (
            SourceChange::Upsert {
                source: source(),
                semantic: None,
            },
            false,
        ),
        (
            SourceChange::Upsert {
                source: source(),
                semantic: Some(SemanticUpdate::Partial(batch())),
            },
            false,
        ),
        (
            SourceChange::Upsert {
                source: source(),
                semantic: Some(SemanticUpdate::Failed {
                    reason: "extraction failed".into(),
                }),
            },
            false,
        ),
        (
            SourceChange::Upsert {
                source: source(),
                semantic: Some(SemanticUpdate::Complete(batch())),
            },
            true,
        ),
        (SourceChange::Delete { source: source() }, true),
    ];
    for (change, replaces) in changes {
        assert_eq!(change.replaces_semantics(), replaces);
        let original = input(change);
        original.validate().expect("input");
        let decoded: SemanticInput =
            serde_json::from_value(serde_json::to_value(&original).expect("wire")).expect("decode");
        assert_eq!(decoded, original);
    }
    let mut invalid = input(SourceChange::Delete { source: source() });
    invalid.update_scope = vec!["other.md".into()];
    assert!(invalid.validate().is_err());
    invalid = input(SourceChange::Delete { source: source() });
    invalid.sources.push(invalid.sources[0].clone());
    assert!(invalid.validate().is_err());
}

#[test]
fn provenance_reuses_evidence_validation_without_treating_episode_ids_as_paths() {
    let locator = EvidenceRef {
        source_id: "episode:meeting".into(),
        raw_digest: "a".repeat(64),
        decoded_digest: "b".repeat(64),
        span: ByteSpan { start: 0, end: 6 },
        reference_id: Some("m1".into()),
    };
    let source = SourceRef {
        source_id: locator.source_id.clone(),
        source_version: "v1".into(),
    };
    let mut evidence = SourceEvidence {
        source,
        locator: Some(locator.clone()),
    };
    evidence.validate().expect("versioned semantic source");
    assert!(
        locator.validate().is_err(),
        "code evidence still requires a workspace path"
    );
    let mention = Mention {
        ref_id: "m1".into(),
        owner_id: "section-1".into(),
        target: id("person"),
        evidence: locator.clone(),
    };
    mention.validate().expect("reuse occurrence evidence");
    let mut invalid_mention = mention;
    invalid_mention.ref_id = "m2".into();
    assert!(invalid_mention.validate().is_err());
    evidence.source.source_version = String::new();
    assert!(evidence.validate().is_err());
    evidence.source.source_version = "v1".into();
    evidence.source.source_id = "different".into();
    assert!(evidence.validate().is_err());
    let invalid = SourceAssociation {
        target: ObjectId::Episode {
            identity: id("event"),
        },
        evidence,
        role: SourceRole::Support,
    };
    assert!(invalid.validate().is_err());
}

#[test]
fn episodes_keep_host_content_and_withdrawn_state_and_require_matching_scope() {
    let episode = Episode {
        identity: id("event-1"),
        kind: "message".into(),
        source: source(),
        context: "project".into(),
        occurred_at_ms: None,
        ingested_at_ms: 123,
        content_ref: "host-record-1".into(),
        state: EpisodeState::Withdrawn,
    };
    let mut request = input(SourceChange::Upsert {
        source: source(),
        semantic: None,
    });
    request.episodes.push(episode.clone());
    request
        .validate()
        .expect("withdrawn record preserves history");
    let mut encoded = serde_json::to_value(&episode).expect("record");
    assert_eq!(
        serde_json::from_value::<Episode>(encoded.clone()).expect("round trip"),
        episode
    );
    encoded["state"] = json!("deleted");
    assert!(serde_json::from_value::<Episode>(encoded).is_err());
    request.episodes[0].identity.space_id = "outside".into();
    assert!(request.validate().is_err());
    request.episodes[0] = episode;
    request.episodes[0].source.source_version = "different-version".into();
    assert!(request.validate().is_err());
    request.episodes[0].source.source_version = source().source_version;
    request.episodes[0].source.source_id = "outside.md".into();
    assert!(request.validate().is_err());
}

#[test]
fn batches_reject_duplicate_facts_wrong_source_versions_and_out_of_scope_corrections() {
    let mut duplicate = entity_input();
    duplicate.id = Some("person".into());
    let mut value = batch();
    value.entities = vec![duplicate.clone(), duplicate];
    assert!(value.validate(&source(), "workspace").is_err());
    value = batch();
    value.facts = vec![fact("f1"), fact("f1")];
    assert!(value.validate(&source(), "workspace").is_err());
    value = batch();
    let mut wrong = evidence();
    wrong.source.source_version = "v0".into();
    value.associations.push(SourceAssociation {
        target: ObjectId::Fact { identity: id("f1") },
        evidence: wrong,
        role: SourceRole::Support,
    });
    assert!(value.validate(&source(), "workspace").is_err());
    let mut value = input(SourceChange::Delete { source: source() });
    let mut outside = evidence();
    outside.source.source_id = "outside.md".into();
    value.fact_changes.push(FactChange::Retract {
        target: id("f1"),
        expected_state: FactState::Active,
        context: "project".into(),
        changed_at_ms: 200,
        evidence: outside,
    });
    assert!(value.validate().is_err());
}
