use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;
use zg_knowledge::{
    processing::{ProcessingContext, canonical_json, stable_digest},
    relations::{SourceVersion, extract_markdown, resolve_documents},
};

fn context() -> ProcessingContext {
    ProcessingContext {
        max_input_bytes: 65536,
        max_output_items: 1000,
        deadline: None,
        cancellation: CancellationToken::new(),
    }
}

pub(crate) fn contract_snapshot() -> Value {
    let inputs = [
        (
            "a.md",
            "\u{feff}前言\r\n# Parent\r\nintro\r\n## 子章节\r\n[x](b.md#hello-world)\r\n```md\r\n# fake\r\n[x](missing.md)\r\n```\r\n\r\n| a | b |\r\n|---|---|\r\n| x | y |\r\n\r\nNext\r\n===\r\nend\r\n",
        ),
        (
            "b.md",
            "# Hello *World*\n[back][a]\n\n[a]: a.md#parent\n# 中文\n# 中文\n",
        ),
    ];
    let facts = inputs.map(|(path, text)| {
        let digest = zg_storage::sha256_hex(text.as_bytes());
        extract_markdown(
            SourceVersion {
                generation: "contract-generation".into(),
                path: path.into(),
                raw_digest: digest.clone(),
                decoded_digest: digest,
            },
            text,
            &context(),
        )
        .expect("source-mapped facts")
    });
    let resolutions = resolve_documents(&facts, &["a.md".into(), "b.md".into()], &context())
        .expect("link resolution");
    let encoding = json!({"z": [3, {"中文": "😀\r\n\"", "a": null}], "a": true});
    json!({
        "canonical": canonical_json(&encoding).expect("canonical bytes"),
        "digest": stable_digest(&encoding).expect("fingerprint"),
        "facts": facts,
        "resolutions": resolutions,
    })
}

#[test]
fn graph_facts_ids_ranges_and_fingerprints_match_pre_consolidation_bytes() {
    let mut expected: Value = serde_json::from_str(include_str!("fixtures/module-contract.json"))
        .expect("frozen pre-consolidation contract");
    // Keep the historical fixture. The new fact contract changes only its rule
    // fingerprint; source IDs, positions and resolved evidence remain stable.
    for file in expected["relations"]["facts"]
        .as_array_mut()
        .expect("facts")
    {
        assert_eq!(
            file["rules_fingerprint"],
            "ad648e1227ab9d944d1aac6ee7f1f0e2e7671ccc854efc8c15854db8751687ca"
        );
        file["rules_fingerprint"] =
            json!("9c3936d25ad1e2f914444e4c2d1fa983809ce5eae8d9e918d3b2f6e489cfba29");
        for node in file["nodes"].as_array_mut().expect("nodes") {
            node["entity_kind"] = node["kind"].clone();
        }
    }
    assert_eq!(contract_snapshot(), expected["relations"]);
}

#[cfg(feature = "graphdb")]
#[test]
fn semantic_changes_v7_schema_is_an_explicit_format_break() {
    let expected: Value = serde_json::from_str(include_str!("fixtures/module-contract.json"))
        .expect("frozen pre-consolidation contract");
    assert_eq!(zg_knowledge::graph::schema::SCHEMA_VERSION, 7);
    assert_ne!(
        zg_knowledge::graph::schema::fingerprint().expect("facts schema"),
        expected["schema"]
    );
    assert_ne!(
        zg_knowledge::graph::schema::build_fingerprint().expect("build schema"),
        expected["build_schema"]
    );
}

#[test]
fn ordinal_row_identity_stays_stable_across_raw_contract_extensions() {
    assert_eq!(
        zg_knowledge::relations::bound::synthetic_id("src/a.rs", "scope", 0)
            .expect("ordinal identity"),
        "843d38bd74ff6cca56a01fae05ab6e46861f16da7caa1ce9520b9f4a535219b1"
    );
    assert_eq!(
        zg_knowledge::relations::bound::synthetic_id("src/a.rs", "binding", 0)
            .expect("ordinal identity"),
        "986b9bae9326ab7cb3eb5e13595fd2a1fb21576713a385f0bf085c9e0c9fd20b"
    );
    assert_eq!(
        zg_knowledge::relations::bound::synthetic_id("src/a.rs", "diagnostic", 0)
            .expect("ordinal identity"),
        "8a6b2ff017ad7156c9cf5bbaf00364d719cce532925fe8ad0c88b18092115155"
    );
    assert_eq!(
        zg_knowledge::relations::bound::synthetic_id("src/a.rs", "anchor", 0)
            .expect("ordinal identity"),
        "127cff9fe995ad0f3aaf738e17aa83417f7ac1950592e10a388d6ad3c3780f21"
    );
}
