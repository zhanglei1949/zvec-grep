use serde::Deserialize;
use zg_knowledge::semantic::{SemanticInput, SourceChange};

#[derive(Deserialize)]
struct Fixtures {
    version: u32,
    cases: Vec<Case>,
}
#[derive(Deserialize)]
struct Case {
    name: String,
    input: SemanticInput,
    expected: Expected,
}
#[derive(Deserialize)]
struct Expected {
    replaces: Vec<String>,
    deletes: Vec<String>,
    fact_changes: usize,
}

#[test]
fn frozen_source_changes_distinguish_omission_failure_deletion_and_explicit_correction() {
    let fixtures: Fixtures =
        serde_json::from_str(include_str!("fixtures/semantic-source-changes-v1.json"))
            .expect("frozen source cases");
    assert_eq!(fixtures.version, 1);
    assert_eq!(fixtures.cases.len(), 13);
    for case in fixtures.cases {
        case.input.validate().expect(&case.name);
        let replaces: Vec<_> = case
            .input
            .sources
            .iter()
            .filter(|change| change.replaces_semantics())
            .map(|change| change.source().source_id.clone())
            .collect();
        let deletes: Vec<_> = case
            .input
            .sources
            .iter()
            .filter(|change| matches!(change, SourceChange::Delete { .. }))
            .map(|change| change.source().source_id.clone())
            .collect();
        assert_eq!(replaces, case.expected.replaces, "{}", case.name);
        assert_eq!(deletes, case.expected.deletes, "{}", case.name);
        assert_eq!(
            case.input.fact_changes.len(),
            case.expected.fact_changes,
            "{}",
            case.name
        );

        for source in &case.input.sources {
            let mut outside = case.input.clone();
            outside
                .update_scope
                .retain(|id| id != &source.source().source_id);
            assert!(outside.validate().is_err(), "{}: scope enforced", case.name);
        }
        let encoded = serde_json::to_value(&case.input).expect("source change wire");
        let decoded: SemanticInput = serde_json::from_value(encoded).expect("source change decode");
        assert_eq!(decoded, case.input, "{}", case.name);
    }
}

#[test]
fn correction_evidence_cannot_disagree_with_the_source_version_in_the_same_input() {
    let fixtures: Fixtures =
        serde_json::from_str(include_str!("fixtures/semantic-source-changes-v1.json"))
            .expect("frozen source cases");
    let mut input = fixtures
        .cases
        .into_iter()
        .find(|case| case.name == "explicit-correction")
        .expect("explicit correction")
        .input;
    if let SourceChange::Upsert { source, .. } = &mut input.sources[0] {
        source.source_version = "different-version".into();
    }
    assert!(input.validate().is_err());
}
