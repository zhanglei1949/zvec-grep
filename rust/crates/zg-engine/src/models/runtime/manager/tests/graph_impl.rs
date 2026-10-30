//! Inherent impls against published native stores; no model acquisition during graph-only or reads.
use super::graph_indexing::*;
use super::*;
use crate::api::explore::*;
use crate::api::index::graph::GraphReady;
use crate::pipelines::indexing::service::WorkspaceIndexService;

fn request(root: &std::path::Path, operation: ExploreOperation) -> ExploreOptions {
    let mut options = ExploreOptions::new(ExploreRequest {
        domain: QueryDomain::Code,
        operation,
        boundary: SourceFilter::default(),
        result_filter: SourceFilter::default(),
    });
    options.root = Some(root.into());
    options
}
fn named(name: &str) -> Locator {
    Locator::Name {
        name: name.into(),
        source_id: None,
        kind: None,
    }
}

#[tokio::test]
async fn inherent_calls_publish_reopen_and_reparse_changed_impls() {
    assert_impl_fixture(
        "pub struct Widget;mod methods;use crate::Widget as Imported;type Alias=Imported;fn impl_caller(){Widget::target();Alias::target();}\n",
        "use crate::Widget as Owner;impl Owner {pub fn target(){Self::leaf();}fn leaf(){}}\n",
        "impl Owner {pub fn target(){}}",
    ).await;
}

#[tokio::test]
async fn lifetime_calls_publish_reopen_and_reparse_changed_impls() {
    assert_impl_fixture(
        "pub struct Widget<'a>(&'a str);mod methods;use crate::Widget as Imported;use Imported as Alias;fn impl_caller(){Widget::target();Alias::target();}\n",
        "use crate::Widget as Owner;impl<'a> Owner<'a> {pub fn target(){Self::leaf();}}impl Owner<'_>{fn leaf(){}}\n",
        "impl Owner<'_> {pub fn target(){}}",
    ).await;
}

async fn assert_impl_fixture(root: &str, methods: &str, duplicate: &str) {
    use crate::api::explore::ResolutionStatus::{Ambiguous, Resolved, Unresolved};
    let dir = tempfile::tempdir().expect("workspace");
    std::fs::create_dir(dir.path().join("src")).expect("source root");
    std::fs::write(dir.path().join("src/lib.rs"), root).expect("types and caller");
    std::fs::write(dir.path().join("src/methods.rs"), methods).expect("out-of-line impl");
    let calls = Arc::new(AtomicUsize::new(0));
    let manager = models(&calls);
    let opts = options(dir.path());
    let service = WorkspaceIndexService::with_test_registry();
    service
        .index(&manager, opts.clone())
        .await
        .expect("text index");
    let home = active(dir.path()).storage_home();
    let text_before = text_hashes(&home.join("storage"));
    let before = calls.load(Ordering::Acquire);
    let zero =
        ModelRuntimeManager::with_factory(|_, _, _| panic!("graph-only must not acquire models"));
    let built = service
        .index(
            &zero,
            crate::api::index::IndexOptions {
                root: Some(dir.path().into()),
                graph: Some(policy()),
                ..Default::default()
            },
        )
        .await
        .expect("graph-only");
    assert_eq!(
        built.graph.expect("graph").domains["code"].ready,
        GraphReady::Ready
    );
    assert_eq!(calls.load(Ordering::Acquire), before);
    assert_eq!(text_hashes(&home.join("storage")), text_before);
    drop(service);
    assert_calls(dir.path(), 2, Resolved).await;
    let engine = crate::ZvecGrep::new();
    let incoming = engine
        .explore(request(
            dir.path(),
            ExploreOperation::Neighbors {
                anchor: named("target"),
                relations: vec![RelationKind::Calls],
                direction: Direction::Incoming,
            },
        ))
        .await
        .expect("callers");
    assert_eq!(incoming.relations.len(), 2);
    let path = engine
        .explore(request(
            dir.path(),
            ExploreOperation::ShortestPath {
                start: named("impl_caller"),
                target: named("leaf"),
                traversal: TraversalSpec::default(),
            },
        ))
        .await
        .expect("Type then Self");
    assert_eq!(path.paths[0].relations.len(), 2);
    assert_eq!(engine.runtime_snapshot().loaded_models, 0);
    engine.close();
    drop(engine);
    let mut update = opts;
    update.graph = Some(policy());
    for (source, count, status) in [
        (
            methods.replace("fn target()", "fn removed()"),
            0,
            Unresolved,
        ),
        (format!("{methods}{duplicate}"), 0, Ambiguous),
        (methods.into(), 2, Resolved),
    ] {
        std::fs::write(dir.path().join("src/methods.rs"), source).expect("change target");
        let service = WorkspaceIndexService::with_test_registry();
        let result = service
            .index(&manager, update.clone())
            .await
            .expect("recompute unchanged caller and alias");
        assert_eq!(
            result.graph.expect("graph").domains["code"].ready,
            GraphReady::Ready
        );
        drop(service);
        assert_calls(dir.path(), count, status).await;
    }
}
async fn assert_calls(
    root: &std::path::Path,
    count: usize,
    status: crate::api::explore::ResolutionStatus,
) {
    let engine = crate::ZvecGrep::new();
    let outgoing = engine
        .explore(request(
            root,
            ExploreOperation::Neighbors {
                anchor: named("impl_caller"),
                relations: vec![RelationKind::Calls],
                direction: Direction::Outgoing,
            },
        ))
        .await
        .expect("reopened callees");
    assert_eq!(outgoing.relations.len(), count);
    let refs = engine
        .explore(request(
            root,
            ExploreOperation::References {
                selector: ReferenceSelector::Owner {
                    owner: named("impl_caller"),
                },
            },
        ))
        .await
        .expect("reopened references");
    assert_eq!(refs.references.len(), 2);
    assert!(refs.references.iter().all(|r| r.status == status));
    assert_eq!(engine.runtime_snapshot().loaded_models, 0);
    engine.close();
}
