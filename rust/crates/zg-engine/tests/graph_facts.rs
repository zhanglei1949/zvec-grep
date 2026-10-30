#![cfg(feature = "knowledge-graph")]
use std::{
    path::PathBuf,
    time::{Duration, Instant},
};
use tokio_util::sync::CancellationToken;
use zg_engine::graph_facts::{
    FactInput, FactInspection, inspect_sources,
    relations::{CodeModule, Language, Lookup, RelationKind, ResolutionStatus},
};
use zg_knowledge::processing::ProcessingContext;

fn context() -> ProcessingContext {
    ProcessingContext {
        max_input_bytes: 1 << 20,
        max_output_items: 10_000,
        deadline: None,
        cancellation: CancellationToken::new(),
    }
}
fn input(path: &str, language: Language, text: &str, module: &[&str]) -> FactInput {
    FactInput {
        path: path.into(),
        language,
        bytes: text.as_bytes().to_vec(),
        module: (language != Language::Markdown).then(|| CodeModule {
            namespace: "fixture".into(),
            path: module.iter().map(|s| (*s).into()).collect(),
            is_package: false,
        }),
    }
}
fn inspect(inputs: &[FactInput]) -> FactInspection {
    inspect_sources("g1", inputs, &context()).expect("facts")
}
fn calls(result: &FactInspection) -> Vec<&zg_knowledge::relations::ResolvedReference> {
    result
        .references
        .iter()
        .filter(|r| r.reference.kind == RelationKind::Call)
        .collect()
}

#[test]
fn rust_group_import_alias_and_lexical_calls() {
    let result = inspect(&[
        input(
            "src/lib.rs",
            Language::Rust,
            "use crate::calc::{add as plus, twice}; fn run() { twice(plus(1,2)); }",
            &[],
        ),
        input(
            "src/calc.rs",
            Language::Rust,
            "pub fn add(a:i32,b:i32)->i32 {a+b} pub fn twice(v:i32)->i32 {add(v,v)}",
            &["calc"],
        ),
    ]);
    let c = calls(&result);
    assert_eq!(c.len(), 3);
    assert!(c.iter().all(|r| r.status == ResolutionStatus::Resolved));
    assert!(c.iter().all(|r| r.candidates[0].path == "src/calc.rs"));
    assert!(
        result
            .references
            .iter()
            .filter(|r| r.reference.kind == RelationKind::Import)
            .all(|r| r.status == ResolutionStatus::Resolved)
    );
}
#[test]
fn python_relative_import_and_module_alias() {
    let result = inspect(&[
        input(
            "pkg/app.py",
            Language::Python,
            "from .calc import add as plus\nimport pkg.calc as c\ndef run():\n    return c.twice(plus(1,2))\n",
            &["pkg", "app"],
        ),
        input(
            "pkg/calc.py",
            Language::Python,
            "def add(a,b): return a+b\ndef twice(v): return add(v,v)\n",
            &["pkg", "calc"],
        ),
    ]);
    assert_eq!(calls(&result).len(), 3);
    assert!(
        calls(&result)
            .iter()
            .all(|r| r.status == ResolutionStatus::Resolved)
    );
}
#[test]
fn parameters_and_assignments_never_resolve_to_global_same_name() {
    for (language, path, source) in [
        (
            Language::Python,
            "a.py",
            "def helper(): pass\ndef run(helper):\n    helper()\ndef other():\n    helper()\n    helper = 2\n",
        ),
        (
            Language::Rust,
            "a.rs",
            "fn helper() {} fn run(helper:fn()) {helper();} fn other() {let helper=||{};helper();}",
        ),
    ] {
        let result = inspect(&[input(path, language, source, &["a"])]);
        assert_eq!(calls(&result).len(), 2);
        assert!(
            calls(&result)
                .iter()
                .all(|r| r.status == ResolutionStatus::Unresolved)
        );
    }
}
#[test]
fn rust_let_initializer_uses_prior_binding_and_blocks_do_not_leak() {
    let result = inspect(&[input(
        "a.rs",
        Language::Rust,
        "fn helper()->i32 {1} fn run(){let helper=helper();helper();} fn other(){helper();}",
        &[],
    )]);
    let states: Vec<_> = calls(&result).iter().map(|r| r.status).collect();
    assert_eq!(
        states,
        [
            ResolutionStatus::Resolved,
            ResolutionStatus::Unresolved,
            ResolutionStatus::Resolved
        ]
    );
}
#[test]
fn duplicate_definitions_are_ambiguous() {
    let result = inspect(&[input(
        "a.py",
        Language::Python,
        "def same():pass\ndef same():pass\nsame()\n",
        &["a"],
    )]);
    assert_eq!(calls(&result)[0].status, ResolutionStatus::Ambiguous);
    assert_eq!(calls(&result)[0].candidates.len(), 2);
}
#[test]
fn same_module_in_two_namespaces_does_not_cross_resolve() {
    let a = input(
        "one/a.py",
        Language::Python,
        "from b import helper\nhelper()\n",
        &["a"],
    );
    let mut b = input("two/b.py", Language::Python, "def helper():pass\n", &["b"]);
    b.module.as_mut().expect("module").namespace = "different-root".into();
    let result = inspect(&[a, b]);
    assert_ne!(calls(&result)[0].status, ResolutionStatus::Resolved);
}
#[test]
fn duplicate_module_files_remain_ambiguous() {
    let result = inspect(&[
        input(
            "app.py",
            Language::Python,
            "from b import helper\nhelper()\n",
            &["app"],
        ),
        input("one/b.py", Language::Python, "def helper():pass\n", &["b"]),
        input("two/b.py", Language::Python, "def helper():pass\n", &["b"]),
    ]);
    assert_eq!(calls(&result)[0].status, ResolutionStatus::Ambiguous);
}
#[test]
fn dynamic_receivers_decorators_and_function_objects_are_conservative() {
    let result = inspect(&[input(
        "a.py",
        Language::Python,
        "def helper(): pass\n@replace\ndef wrapped(): pass\ndef run(obj):\n    obj.helper()\n    helper()()\n    wrapped()\n",
        &["a"],
    )]);
    let c = calls(&result);
    assert_eq!(
        c.iter()
            .filter(|r| r.status == ResolutionStatus::Resolved)
            .count(),
        1
    );
    assert_eq!(
        c.iter()
            .filter(|r| r.status == ResolutionStatus::Unresolved)
            .count(),
        3
    );
}
#[test]
fn python_method_unqualified_name_skips_class_scope() {
    let result = inspect(&[input(
        "a.py",
        Language::Python,
        "def helper():pass\nclass C:\n    def helper():pass\n    def run(self):\n        helper()\n",
        &["a"],
    )]);
    let c = calls(&result);
    assert_eq!(c[0].status, ResolutionStatus::Resolved);
    let f = &result.files[0];
    let n = f
        .nodes
        .iter()
        .find(|n| n.id == c[0].candidates[0].node_id)
        .expect("target");
    assert_eq!(n.qualified_name, ["helper"]);
}
#[test]
fn parse_errors_and_unsupported_binding_forms_disable_resolution() {
    for source in [
        "def helper(): pass\ndef broken(:\nhelper()\n",
        "from b import *\ndef run(): helper()\n",
    ] {
        let result = inspect(&[input("a.py", Language::Python, source, &["a"])]);
        assert!(!result.files[0].complete);
        assert!(
            result
                .references
                .iter()
                .all(|r| r.status != ResolutionStatus::Resolved)
        );
    }
}
#[test]
fn body_and_line_changes_preserve_logical_identity_but_not_version() {
    let a = inspect(&[input("a.rs", Language::Rust, "fn stable(){let x=1;}", &[])]);
    let b = inspect(&[input(
        "a.rs",
        Language::Rust,
        "\n\nfn stable(){let x=2;}",
        &[],
    )]);
    let named = |r: &FactInspection| {
        r.files[0]
            .nodes
            .iter()
            .find(|n| n.name == "stable")
            .expect("stable")
            .id
            .clone()
    };
    assert_eq!(named(&a), named(&b));
    assert_ne!(a.files[0].version, b.files[0].version);
    let c = inspect(&[input("a.rs", Language::Rust, "fn renamed(){let x=2;}", &[])]);
    assert_ne!(named(&a), c.files[0].nodes[1].id);
}
#[test]
fn target_edits_and_removal_change_resolution_and_read_set() {
    let source = input(
        "a.py",
        Language::Python,
        "from b import helper\nhelper()\n",
        &["a"],
    );
    let target = input("b.py", Language::Python, "def helper():pass\n", &["b"]);
    let before = inspect(&[source.clone(), target.clone()]);
    let deleted = inspect(std::slice::from_ref(&source));
    let renamed = inspect(&[
        source.clone(),
        input("b.py", Language::Python, "def renamed():pass\n", &["b"]),
    ]);
    let restored = inspect(&[source, target]);
    assert_eq!(calls(&before)[0].status, ResolutionStatus::Resolved);
    assert_ne!(calls(&deleted)[0].status, ResolutionStatus::Resolved);
    assert_ne!(calls(&renamed)[0].status, ResolutionStatus::Resolved);
    assert_eq!(before.references, restored.references);
    assert_eq!(calls(&before)[0].dependencies.len(), 2);
}
#[test]
fn markdown_links_anchors_backlinks_and_code_boundary() {
    let result = inspect(&[
        input(
            "docs/a.md",
            Language::Markdown,
            "# 入口\n[link](b.md#build) [code](../a.rs) [back](#入口)\n",
            &[],
        ),
        input(
            "docs/b.md",
            Language::Markdown,
            "# Build\n[back](a.md#入口)\n",
            &[],
        ),
        input("a.rs", Language::Rust, "fn f(){}", &[]),
    ]);
    let states: Vec<_> = result.references.iter().map(|r| r.status).collect();
    assert_eq!(
        states,
        [
            ResolutionStatus::Resolved,
            ResolutionStatus::Unsupported,
            ResolutionStatus::Resolved,
            ResolutionStatus::Resolved
        ]
    );
    assert!(
        result
            .references
            .iter()
            .filter(|r| r.status == ResolutionStatus::Resolved)
            .all(|r| std::path::Path::new(&r.candidates[0].path)
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("md")))
    );
    assert!(
        result
            .references
            .iter()
            .all(
                |r| r.dependencies.iter().all(|d| std::path::Path::new(&d.path)
                    .extension()
                    .is_some_and(|e| e.eq_ignore_ascii_case("md")))
            )
    );
}
#[test]
fn markdown_duplicate_anchor_external_escape_and_missing_target() {
    let result = inspect(&[input(
        "a.md",
        Language::Markdown,
        "# Same\n# Same\n[x](#same) [e](https://example.com) [escape](../a.md) [anchor](#absent) [missing](b.md) [encoded](%2Ftmp/a.md)\n",
        &[],
    )]);
    let states: Vec<_> = result.references.iter().map(|r| r.status).collect();
    assert_eq!(
        states,
        [
            ResolutionStatus::Ambiguous,
            ResolutionStatus::External,
            ResolutionStatus::OutsideWorkspace,
            ResolutionStatus::Unresolved,
            ResolutionStatus::Unresolved,
            ResolutionStatus::OutsideWorkspace
        ]
    );
    assert!(
        result.files[0]
            .diagnostics
            .iter()
            .any(|d| d.code == "duplicate_heading_anchor")
    );
}
#[test]
fn commonmark_reference_links_ignore_code_fences_and_map_decoded_crlf_bom() {
    let source = "\u{feff}# 中文\r\n[读][id]\r\n\r\n[id]: #中文\r\n\r\n```md\r\n[fake](missing.md)\r\n```\r\n";
    let result = inspect(&[input("a.md", Language::Markdown, source, &[])]);
    assert_eq!(result.references.len(), 1);
    assert_eq!(result.references[0].status, ResolutionStatus::Resolved);
    let s = result.references[0].reference.span;
    assert_eq!(
        &source.trim_start_matches('\u{feff}')[s.start..s.end],
        "[读][id]"
    );
}
#[test]
fn python_latin_one_uses_existing_decoded_utf8_coordinates() {
    let mut i = input("a.py", Language::Python, "", &["a"]);
    i.bytes = b"# coding: latin-1\n# caf\xe9\ndef f(): pass\nf()\n".to_vec();
    let result = inspect(&[i]);
    assert_eq!(calls(&result)[0].status, ResolutionStatus::Resolved);
    let decoded = "# coding: latin-1\n# café\ndef f(): pass\nf()\n";
    let span = calls(&result)[0].reference.span;
    assert_eq!(&decoded[span.start..span.end], "f");
    assert_eq!(span.start, decoded.rfind("f()").expect("call site"));
    assert_ne!(
        result.files[0].version.raw_digest,
        result.files[0].version.decoded_digest
    );
}
#[test]
fn context_cancel_deadline_budget_and_duplicate_paths_are_errors() {
    let source = input("a.rs", Language::Rust, "fn a(){a();}", &[]);
    let mut c = context();
    c.cancellation.cancel();
    assert_eq!(
        inspect_sources("g1", std::slice::from_ref(&source), &c)
            .expect_err("cancelled")
            .code(),
        zg_engine::EngineError::CANCELLED
    );
    c = context();
    c.deadline = Instant::now().checked_sub(Duration::from_secs(1));
    assert_eq!(
        inspect_sources("g1", std::slice::from_ref(&source), &c)
            .expect_err("deadline")
            .code(),
        zg_engine::EngineError::DEADLINE_EXCEEDED
    );
    c = context();
    c.max_output_items = 1;
    assert!(inspect_sources("g1", std::slice::from_ref(&source), &c).is_err());
    assert!(inspect_sources("g1", &[source.clone(), source], &context()).is_err());
    assert!(
        inspect_sources(
            "g1",
            &[input("../a.md", Language::Markdown, "text", &[])],
            &context()
        )
        .is_err()
    );
}

fn fixture_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../benchmarks/zg-graph")
}
#[test]
fn frozen_sixteen_questions_have_exact_evidence_and_expected_candidate_facts() {
    let root = fixture_root();
    let frozen: serde_json::Value = serde_json::from_slice(
        &std::fs::read(root.join("frozen-inputs.json")).expect("frozen manifest"),
    )
    .expect("JSON");
    for file in ["sources.json", "questions.json"] {
        assert_eq!(
            zg_storage::sha256_hex(&std::fs::read(root.join(file)).expect("frozen file")),
            frozen["files"][file].as_str().expect("digest")
        );
    }
    let manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(root.join("sources.json")).expect("sources"))
            .expect("JSON");
    let inputs: Vec<_> = manifest["sources"]
        .as_array()
        .expect("array")
        .iter()
        .map(|e| {
            let path = e["path"].as_str().expect("path");
            let bytes = std::fs::read(root.join("corpus").join(path)).expect("source");
            assert_eq!(
                zg_storage::sha256_hex(&bytes),
                e["sha256"].as_str().expect("digest")
            );
            FactInput {
                path: path.into(),
                bytes,
                language: serde_json::from_value(e["language"].clone()).expect("language"),
                module: if e["module"].is_null() {
                    None
                } else {
                    Some(serde_json::from_value(e["module"].clone()).expect("module"))
                },
            }
        })
        .collect();
    let result = inspect_sources("fixture-v1", &inputs, &context()).expect("fixture facts");
    let questions: serde_json::Value =
        serde_json::from_slice(&std::fs::read(root.join("questions.json")).expect("questions"))
            .expect("JSON");
    let questions = questions["questions"].as_array().expect("array");
    assert_eq!(questions.len(), 16);
    for domain in ["code", "document"] {
        assert_eq!(
            questions.iter().filter(|q| q["domain"] == domain).count(),
            8
        );
    }
    for q in questions {
        for e in q["required_evidence"].as_array().expect("evidence") {
            let bytes = std::fs::read(root.join("corpus").join(e["path"].as_str().expect("path")))
                .expect("evidence source");
            assert_eq!(
                zg_storage::sha256_hex(&bytes),
                e["sha256"].as_str().expect("digest")
            );
            let start = usize::try_from(e["start"].as_u64().expect("start")).expect("offset");
            let end = usize::try_from(e["end"].as_u64().expect("end")).expect("offset");
            assert_eq!(
                std::str::from_utf8(&bytes[start..end]).expect("UTF8"),
                e["quote"].as_str().expect("quote")
            );
        }
        let source = q["source"].as_str().expect("source");
        let bytes = &inputs
            .iter()
            .find(|i| i.path == source)
            .expect("source input")
            .bytes;
        let text = std::str::from_utf8(bytes).expect("UTF8");
        let needle = q["reference_text"].as_str().expect("needle");
        let offset = text.find(needle).expect("reference location");
        let reference = result
            .references
            .iter()
            .find(|r| {
                r.source.path == source
                    && r.reference.span.start == offset
                    && !matches!(r.reference.lookup, Lookup::Import { .. })
            })
            .unwrap_or_else(|| panic!("missing reference {}", q["id"]));
        let expected: ResolutionStatus =
            serde_json::from_value(q["expected_status"].clone()).expect("status");
        assert_eq!(reference.status, expected, "{}", q["id"]);
        if let Some(target) = q["target"].as_str() {
            assert_eq!(reference.candidates[0].path, target, "{}", q["id"]);
        }
    }
}

#[test]
fn imports_of_external_targets_stay_external_and_static_method_receivers_do_not_guess() {
    let result = inspect(&[
        input(
            "a.py",
            Language::Python,
            "import unknown as external\ndef f(): return external.run()\n",
            &["a"],
        ),
        input(
            "a.rs",
            Language::Rust,
            "fn f(x: Thing){x.run(); std::unknown();}",
            &[],
        ),
    ]);
    let c = calls(&result);
    assert_eq!(
        c.iter()
            .filter(|r| r.status == ResolutionStatus::External)
            .count(),
        2
    );
    assert_eq!(
        c.iter()
            .filter(|r| r.status == ResolutionStatus::Unresolved)
            .count(),
        1
    );
}
#[test]
fn module_import_targets_the_module_file_and_entity_mapping_is_inspectable() {
    let result = inspect(&[
        input(
            "src/lib.rs",
            Language::Rust,
            "mod calc; use crate::calc as c; fn run(){c::helper();}",
            &[],
        ),
        input(
            "src/calc.rs",
            Language::Rust,
            "pub fn helper() {}",
            &["calc"],
        ),
    ]);
    assert!(
        result
            .references
            .iter()
            .all(|r| r.status == ResolutionStatus::Resolved)
    );
    assert_eq!(calls(&result)[0].candidates[0].path, "src/calc.rs");
    assert!(!calls(&result)[0].reference.entity_indices.is_empty());
    let target = &result.files[1].nodes[1];
    assert!(!target.entity_indices.is_empty());
}

#[test]
fn rust_module_declarations_and_inline_relative_paths_resolve() {
    let result = inspect(&[
        input(
            "src/lib.rs",
            Language::Rust,
            "mod calc; fn root(){calc::helper();} mod inner {fn local(){} fn run(){self::local();super::root();}}",
            &[],
        ),
        input(
            "src/calc.rs",
            Language::Rust,
            "pub fn helper(){}",
            &["calc"],
        ),
    ]);
    assert_eq!(calls(&result).len(), 3);
    assert!(
        calls(&result)
            .iter()
            .all(|r| r.status == ResolutionStatus::Resolved)
    );
}

#[test]
fn imported_decorated_functions_and_function_pointer_constants_do_not_become_static_calls() {
    let result = inspect(&[
        input("a.py", Language::Python, "import b\nb.changed()\n", &["a"]),
        input(
            "b.py",
            Language::Python,
            "@decorator\ndef changed():pass\n",
            &["b"],
        ),
        input(
            "a.rs",
            Language::Rust,
            "const PTR: fn() = helper; fn helper(){} fn run(){PTR();}",
            &[],
        ),
    ]);
    assert_eq!(calls(&result).len(), 2);
    assert!(
        calls(&result)
            .iter()
            .all(|r| r.status == ResolutionStatus::Unresolved)
    );
}

#[test]
fn rust_impl_members_do_not_shadow_free_function_lookup() {
    let result = inspect(&[input(
        "a.rs",
        Language::Rust,
        "fn helper(){} struct A; impl A { fn helper(){} fn run(){helper();} }",
        &[],
    )]);
    let c = calls(&result);
    assert_eq!(c.len(), 1);
    assert_eq!(c[0].status, ResolutionStatus::Resolved);
    let target = result.files[0]
        .nodes
        .iter()
        .find(|n| n.id == c[0].candidates[0].node_id)
        .expect("definition");
    assert_eq!(target.qualified_name, ["helper"]);
}
