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
    // v2 adds semantic kinds; preserve the frozen v1 IDs, ranges and all other bytes.
    for file in expected["relations"]["facts"]
        .as_array_mut()
        .expect("facts")
    {
        for node in file["nodes"].as_array_mut().expect("nodes") {
            node["entity_kind"] = node["kind"].clone();
        }
    }
    assert_eq!(contract_snapshot(), expected["relations"]);
}

#[cfg(feature = "graphdb")]
#[test]
fn construction_v4_schema_is_an_explicit_format_break() {
    let expected: Value = serde_json::from_str(include_str!("fixtures/module-contract.json"))
        .expect("frozen pre-consolidation contract");
    assert_eq!(zg_knowledge::graph::schema::SCHEMA_VERSION, 4);
    assert_ne!(
        zg_knowledge::graph::schema::fingerprint().expect("facts schema"),
        expected["schema"]
    );
    assert_ne!(
        zg_knowledge::graph::schema::build_fingerprint().expect("build schema"),
        expected["build_schema"]
    );
}
