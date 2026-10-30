use super::*;
use crate::{
    domain::{FileId, FileIndexStatus, FileSnapshot, SourcePath},
    graph_facts::{
        FactInput,
        binding::{BindingOptions, BoundInput, inspect_bound_sources},
    },
};

fn context() -> ProcessingContext {
    ProcessingContext {
        max_input_bytes: 1 << 20,
        max_output_items: 10_000,
        deadline: None,
        cancellation: tokio_util::sync::CancellationToken::new(),
    }
}
fn fixture(text: &str) -> (FileRecord, BoundFileFacts) {
    let context = context();
    let mut bound = inspect_bound_sources(
        "coverage",
        1,
        &[BoundInput {
            file_id: 1,
            input: FactInput {
                path: "src/lib.rs".into(),
                bytes: text.as_bytes().to_vec(),
                language: Language::Rust,
                module: Some(zg_knowledge::relations::CodeModule {
                    namespace: "fixture".into(),
                    path: vec![],
                    is_package: false,
                }),
            },
        }],
        BindingOptions::default(),
        &context,
    )
    .expect("facts")
    .remove(0);
    bound.record.index_status = zg_knowledge::relations::bound::IndexStatus::Indexed;
    let file = FileRecord {
        id: FileId::new(1),
        relative_path: SourcePath::new("src/lib.rs").expect("path"),
        snapshot: FileSnapshot {
            size_bytes: text.len() as u64,
            modified_epoch_ms: None,
            content_hash: Some(crate::utils::sha256_hex(text.as_bytes())),
        },
        index_status: FileIndexStatus::Indexed {
            indexed_epoch_ms: 1,
            entity_count: bound.record.entity_count,
        },
    };
    (file, bound)
}
fn roots() -> ModuleRoots {
    ModuleRoots::new(vec![crate::graph_facts::module_roots::ModuleRoot {
        root: "src".into(),
        language: Language::Rust,
        namespace: "fixture".into(),
        module_prefix: vec![],
        layout: crate::graph_facts::module_roots::PackageLayout::Flat,
    }])
    .expect("roots")
}

#[test]
fn graph_coverage_distinguishes_missing_mapping_missing_facts_and_syntax_errors() {
    let (file, mut bound) = fixture("fn ok() {}\n");
    let empty = BTreeSet::new();
    assert_eq!(
        source_state(&file, Language::Rust, &roots(), Some(&bound), &empty)
            .expect("valid coverage fixture")
            .state,
        GraphSourceState::Ready
    );
    assert_eq!(
        source_state(
            &file,
            Language::Rust,
            &ModuleRoots::new(vec![]).expect("empty roots"),
            Some(&bound),
            &empty
        )
        .expect("valid coverage fixture")
        .state,
        GraphSourceState::NoModuleRoot
    );
    assert_eq!(
        source_state(&file, Language::Rust, &roots(), None, &empty)
            .expect("valid coverage fixture")
            .state,
        GraphSourceState::FactsMissing
    );
    bound.facts.as_mut().expect("fixture facts").complete = false;
    assert_eq!(
        source_state(&file, Language::Rust, &roots(), Some(&bound), &empty)
            .expect("valid coverage fixture")
            .state,
        GraphSourceState::IncompleteFacts
    );
    let (file, broken) = fixture("fn broken( {\n");
    let state = source_state(&file, Language::Rust, &roots(), Some(&broken), &empty)
        .expect("valid coverage fixture");
    assert_eq!(state.state, GraphSourceState::IncompleteFacts);
    assert!(state.diagnostics.iter().any(|d| d == "syntax_error"));
}

#[test]
fn graph_coverage_requires_resolution_execution_but_not_a_resolved_target() {
    let (file, bound) = fixture("fn caller() { unknown_target(); }\n");
    let facts = bound.facts.as_ref().expect("fixture facts");
    assert_eq!(facts.references.len(), 1);
    assert_eq!(
        source_state(
            &file,
            Language::Rust,
            &roots(),
            Some(&bound),
            &BTreeSet::new()
        )
        .expect("valid coverage fixture")
        .state,
        GraphSourceState::ResolutionPending
    );
    let catalog = FactCatalog::resolve(vec![bound], "a".repeat(64), "b".repeat(64), &context())
        .expect("valid coverage fixture");
    assert_ne!(
        catalog.resolutions[0].status,
        zg_knowledge::relations::ResolutionStatus::Resolved
    );
    let processed = catalog
        .resolutions
        .iter()
        .map(|r| (r.file_path.as_str(), r.reference_id.as_str()))
        .collect();
    assert_eq!(
        source_state(
            &file,
            Language::Rust,
            &roots(),
            Some(&catalog.files[0]),
            &processed
        )
        .expect("valid coverage fixture")
        .state,
        GraphSourceState::Ready
    );
}
