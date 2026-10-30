#![cfg(feature = "knowledge-graph")]
use std::path::PathBuf;
use zg_engine::graph_facts::{
    FactInput,
    binding::{BindingOptions, BoundInput, inspect_bound_sources},
    module_roots::{ModuleMapping, ModuleRoot, ModuleRoots, PackageLayout},
    relations::{CodeModule, Language, bound::synthetic_id},
};
use zg_knowledge::processing::ProcessingContext;

fn context() -> ProcessingContext {
    ProcessingContext {
        max_input_bytes: 1 << 20,
        max_output_items: 50_000,
        deadline: None,
        cancellation: tokio_util::sync::CancellationToken::new(),
    }
}
fn input(path: &str, language: Language, bytes: Vec<u8>, id: u32) -> BoundInput {
    BoundInput {
        file_id: id,
        input: FactInput {
            path: path.into(),
            language,
            bytes,
            module: (language != Language::Markdown).then(|| CodeModule {
                namespace: "fixture".into(),
                path: vec!["a".into()],
                is_package: false,
            }),
        },
    }
}

#[test]
fn ordinal_ids_and_structure_ignore_retrieval_chunk_configuration() {
    let rust = format!(
        "fn helper() {{}}\nfn run() {{ helper(); {} }}",
        "let x=1;".repeat(100)
    );
    let inputs = vec![
        input("a.rs", Language::Rust, rust.into_bytes(), 9),
        input(
            "a.py",
            Language::Python,
            b"def helper(): pass\ndef run():\n    helper()\n".to_vec(),
            10,
        ),
        input(
            "a.md",
            Language::Markdown,
            b"# Same\n# Same\n[x](#same)\n".to_vec(),
            11,
        ),
    ];
    let a = inspect_bound_sources("g", 1, &inputs, BindingOptions::default(), &context())
        .expect("default");
    let repeat = inspect_bound_sources("g", 1, &inputs, BindingOptions::default(), &context())
        .expect("repeat");
    assert_eq!(a, repeat);
    let b = inspect_bound_sources(
        "g",
        1,
        &inputs,
        BindingOptions {
            max_chunk_chars: Some(128),
            chunk_overlap_chars: Some(16),
        },
        &context(),
    )
    .expect("different chunks");
    for (a, b) in a.iter().zip(&b) {
        let fa = a.facts.as_ref().expect("facts");
        let fb = b.facts.as_ref().expect("facts");
        assert_eq!(fa, fb, "structure includes no temporary entity indices");
        for (kind, n) in [
            ("scope", fa.scopes.len()),
            ("binding", fa.bindings.len()),
            ("diagnostic", fa.diagnostics.len()),
        ] {
            for i in 0..n {
                assert_eq!(
                    synthetic_id(&fa.version.path, kind, i).expect("ID"),
                    synthetic_id(&fb.version.path, kind, i).expect("ID")
                );
            }
        }
        for ids in a.node_entities.iter().chain(&a.reference_entities) {
            for id in ids {
                assert!(id.starts_with(&format!("{:08x}", a.record.file_id)));
            }
        }
        assert_eq!(
            a.record.index_status,
            zg_knowledge::relations::bound::IndexStatus::NotIndexed,
            "inspection does not claim text success"
        );
    }
    // Semantic entities retain full content; chunk limits change Fragments,
    // so EntityIds may correctly remain identical. Still reject invalid
    // nondefault chunk options to prove the caller's options reach extraction.
    assert!(
        inspect_bound_sources(
            "g",
            1,
            &inputs,
            BindingOptions {
                max_chunk_chars: Some(8),
                chunk_overlap_chars: Some(8)
            },
            &context()
        )
        .is_err()
    );
}

#[test]
fn raw_and_decoded_coordinates_handle_bom_crlf_and_python_cookie() {
    let mut bom = vec![0xef, 0xbb, 0xbf];
    bom.extend_from_slice("# 中文\r\n[x](#中文)\r\n".as_bytes());
    let cookie = b"# coding: latin-1\r\ndef caf\xe9(): pass\r\ncaf\xe9()\r\n".to_vec();
    let inputs = vec![
        input("a.md", Language::Markdown, bom, 1),
        input("a.py", Language::Python, cookie, 2),
    ];
    let files = inspect_bound_sources("g", 1, &inputs, BindingOptions::default(), &context())
        .expect("decoded facts");
    for file in files {
        let facts = file.facts.expect("facts");
        assert_ne!(facts.version.raw_digest, facts.version.decoded_digest);
        assert!(facts.nodes[0].span.end > 0);
    }
}

#[test]
fn malformed_parse_and_missing_mapping_do_not_become_successful_relations() {
    let bad = input("a.rs", Language::Rust, b"fn run( { helper();".to_vec(), 1);
    let output = inspect_bound_sources(
        "g",
        1,
        std::slice::from_ref(&bad),
        BindingOptions::default(),
        &context(),
    )
    .expect("incomplete facts");
    let f = output[0].facts.as_ref().expect("facts");
    assert!(!f.complete);
    assert!(!f.diagnostics.is_empty());
    let mut missing = bad;
    missing.input.module = None;
    assert!(
        inspect_bound_sources("g", 1, &[missing], BindingOptions::default(), &context()).is_err()
    );
    let cancelled = context();
    cancelled.cancellation.cancel();
    assert!(inspect_bound_sources("g", 1, &[], BindingOptions::default(), &cancelled).is_err());
}

fn root(path: &str, language: Language, namespace: &str, layout: PackageLayout) -> ModuleRoot {
    ModuleRoot {
        root: PathBuf::from(path),
        language,
        namespace: namespace.into(),
        module_prefix: Vec::new(),
        layout,
    }
}
#[test]
fn roots_validate_normalization_namespaces_language_and_longest_match() {
    let rust = root("rust", Language::Rust, "outer", PackageLayout::Flat);
    let nested = root(
        "rust/pkg/src",
        Language::Rust,
        "inner",
        PackageLayout::ModRs,
    );
    let roots = ModuleRoots::new(vec![rust.clone(), nested.clone()]).expect("roots");
    assert_eq!(
        roots.fingerprint(),
        ModuleRoots::new(vec![nested, rust.clone()])
            .expect("sorted")
            .fingerprint()
    );
    assert_eq!(
        roots
            .map("rust/pkg/src/util/mod.rs", Language::Rust)
            .expect("map"),
        ModuleMapping::Mapped(CodeModule {
            namespace: "inner".into(),
            path: vec!["util".into()],
            is_package: false
        })
    );
    assert_eq!(
        roots.map("rust/a.py", Language::Python).expect("mismatch"),
        ModuleMapping::LanguageMismatch
    );
    assert_eq!(
        roots.map("else/a.rs", Language::Rust).expect("missing"),
        ModuleMapping::Missing
    );
    assert!(roots.map("rust/../a.rs", Language::Rust).is_err());
    for r in [
        root("../escape", Language::Rust, "x", PackageLayout::Flat),
        root("/outside", Language::Python, "x", PackageLayout::Flat),
        root("docs", Language::Markdown, "x", PackageLayout::Flat),
        root("r", Language::Rust, "", PackageLayout::Flat),
    ] {
        assert!(ModuleRoots::new(vec![r]).is_err());
    }
    assert!(ModuleRoots::new(vec![rust.clone(), rust]).is_err());
    let py = ModuleRoots::new(vec![root(
        "python",
        Language::Python,
        "py",
        PackageLayout::Src,
    )])
    .expect("py");
    assert_eq!(
        py.map("python/src/pkg/__init__.py", Language::Python)
            .expect("package"),
        ModuleMapping::Mapped(CodeModule {
            namespace: "py".into(),
            path: vec!["pkg".into()],
            is_package: true
        })
    );
}

#[cfg(unix)]
#[test]
fn module_roots_reject_real_path_escape() {
    let workspace = tempfile::tempdir().expect("workspace");
    let outside = tempfile::tempdir().expect("outside");
    std::os::unix::fs::symlink(outside.path(), workspace.path().join("escape")).expect("symlink");
    let roots = ModuleRoots::new(vec![root(
        "escape",
        Language::Rust,
        "escape",
        PackageLayout::Flat,
    )])
    .expect("lexical config");
    assert!(roots.validate_workspace(workspace.path()).is_err());
    let roots = ModuleRoots::new(vec![root(
        "",
        Language::Rust,
        "workspace",
        PackageLayout::Flat,
    )])
    .expect("workspace config");
    roots
        .validate_workspace(workspace.path())
        .expect("within workspace");
}

#[test]
fn python_extension_aliases_preserve_module_and_package_identity() {
    let roots = ModuleRoots::new(vec![root(
        "python",
        Language::Python,
        "py",
        PackageLayout::Src,
    )])
    .expect("Python root");
    for extension in ["py", "pyw", "pyi"] {
        for (name, path, is_package) in [
            ("worker", vec!["pkg".into(), "worker".into()], false),
            ("__init__", vec!["pkg".into()], true),
        ] {
            assert_eq!(
                roots
                    .map(
                        &format!("python/src/pkg/{name}.{extension}"),
                        Language::Python
                    )
                    .expect("host-supported Python extension"),
                ModuleMapping::Mapped(CodeModule {
                    namespace: "py".into(),
                    path,
                    is_package,
                })
            );
        }
    }
    assert!(
        roots
            .map("python/src/pkg/worker.txt", Language::Python)
            .is_err()
    );
}

#[cfg(feature = "knowledge-graph-graphdb")]
fn code_change_catalog(
    language: Language,
    target_name: &str,
    imported_name: &str,
    shadowed: bool,
) -> zg_knowledge::relations::bound::FactCatalog {
    use zg_knowledge::{
        processing::stable_digest,
        relations::bound::{FactCatalog, IndexStatus},
    };
    let (extension, caller, target) = match language {
        Language::Rust => (
            "rs",
            format!(
                "use crate::target::{imported_name} as selected;\nfn run({}) {{ selected(); }}\n",
                if shadowed { "selected: fn()" } else { "" }
            ),
            format!("pub fn {target_name}() {{}}\npub fn alternate() {{}}\n"),
        ),
        Language::Python => (
            "py",
            format!(
                "from target import {imported_name} as selected\ndef run({}):\n    selected()\n",
                if shadowed { "selected" } else { "" }
            ),
            format!("def {target_name}(): pass\ndef alternate(): pass\n"),
        ),
        Language::Markdown => panic!("code fixture requires a code language"),
    };
    let inputs = [("caller", caller, 1), ("target", target, 2)]
        .into_iter()
        .map(|(module, text, id)| {
            let mut item = input(
                &format!("{module}.{extension}"),
                language,
                text.into_bytes(),
                id,
            );
            item.input.module.as_mut().expect("code module").path = vec![module.into()];
            item
        })
        .collect::<Vec<_>>();
    let mut files = inspect_bound_sources("g", 1, &inputs, BindingOptions::default(), &context())
        .expect("real host parser and identity binding");
    // Synthetic eligibility isolates persistence/resolution from text writes;
    // actual indexing eligibility is covered by the pipeline's dual-store test.
    for file in &mut files {
        file.record.index_status = IndexStatus::Indexed;
    }
    FactCatalog::resolve(
        files,
        stable_digest(&"code-change-config").expect("config"),
        stable_digest(&"code-change-selection").expect("selection"),
        &context(),
    )
    .expect("resolve fixture")
}

#[cfg(feature = "knowledge-graph-graphdb")]
fn assert_code_changes_resolve_from_reopened_facts(language: Language) {
    use zg_knowledge::{
        graph::{GraphDbStore, GraphLimits},
        relations::{RelationKind, ResolutionStatus, bound::FactCatalog},
    };
    let baseline = code_change_catalog(language, "helper", "helper", false);
    let caller = &baseline.files[0];
    let target = &baseline.files[1];
    let call_id = &caller
        .facts
        .as_ref()
        .expect("caller facts")
        .references
        .iter()
        .find(|r| r.kind == RelationKind::Call)
        .expect("alias call")
        .id;
    let original = baseline
        .resolutions
        .iter()
        .find(|r| &r.reference_id == call_id)
        .expect("original call");
    assert_eq!(original.status, ResolutionStatus::Resolved);
    assert_eq!(original.targets.len(), 1);
    assert_eq!(original.targets[0].source.path, target.record.path);

    let directory = tempfile::tempdir().expect("graph directory");
    let path = directory.path().join("graph");
    let store = GraphDbStore::create(&path, GraphLimits::default()).expect("real graphdb");
    store
        .write_facts(&baseline, &context())
        .expect("persist facts");
    store.checkpoint().expect("checkpoint");
    drop(store);
    let store = GraphDbStore::open(&path, true, GraphLimits::default()).expect("read-only reopen");
    let reopened = store
        .read_facts(&context())
        .expect("decode persisted structure");
    assert_eq!(reopened, baseline);

    for (name, imported, shadowed, changed, expected) in [
        ("renamed", "helper", false, 1, ResolutionStatus::Unresolved),
        ("helper", "alternate", false, 0, ResolutionStatus::Resolved),
        ("helper", "helper", true, 0, ResolutionStatus::Unresolved),
    ] {
        // Full parsing is only the oracle and replacement-file producer. The
        // unchanged source uses its reopened scopes, bindings and EntityIds.
        let fresh = code_change_catalog(language, name, imported, shadowed);
        let mut reused = reopened.files.clone();
        reused[changed] = fresh.files[changed].clone();
        let rerun = FactCatalog::resolve(
            reused,
            reopened.config_fingerprint.clone(),
            reopened.selection_fingerprint.clone(),
            &context(),
        )
        .expect("resolve persisted facts with one changed file");
        assert_eq!(rerun, fresh, "graph reuse must match full parsing");
        assert_eq!(rerun.files[1 - changed], reopened.files[1 - changed]);
        assert_ne!(rerun.catalogs, reopened.catalogs);
        let current_call_id = &rerun.files[0]
            .facts
            .as_ref()
            .expect("caller facts")
            .references
            .iter()
            .find(|r| r.kind == RelationKind::Call)
            .expect("current alias call")
            .id;
        // References are version-scoped evidence, unlike logical symbol IDs.
        if changed == 1 {
            assert_eq!(current_call_id, call_id);
        } else {
            assert_ne!(current_call_id, call_id);
        }
        let call = rerun
            .resolutions
            .iter()
            .find(|r| &r.reference_id == current_call_id)
            .expect("current version call");
        assert_eq!(call.status, expected);
        assert_ne!(call, original);
        if expected == ResolutionStatus::Resolved {
            let alternate = rerun.files[1]
                .facts
                .as_ref()
                .expect("target facts")
                .nodes
                .iter()
                .find(|n| n.name == "alternate")
                .expect("redirected definition");
            assert_eq!(call.targets.len(), 1);
            assert_eq!(call.targets[0].node_id, alternate.id);
            assert_ne!(call.targets[0].node_id, original.targets[0].node_id);
        } else {
            assert!(call.targets.is_empty(), "old unique edge must disappear");
        }
        if shadowed {
            assert_eq!(call.reason, "shadowed_or_dynamic_binding");
        }
    }
    assert_eq!(
        store.read_facts(&context()).expect("unchanged candidate"),
        baseline
    );
}

#[cfg(feature = "knowledge-graph-graphdb")]
#[test]
fn rust_code_changes_resolve_from_reopened_scopes_and_bindings() {
    assert_code_changes_resolve_from_reopened_facts(Language::Rust);
}

#[cfg(feature = "knowledge-graph-graphdb")]
#[test]
fn python_code_changes_resolve_from_reopened_scopes_and_bindings() {
    assert_code_changes_resolve_from_reopened_facts(Language::Python);
}
