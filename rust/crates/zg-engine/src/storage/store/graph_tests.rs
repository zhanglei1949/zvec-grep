//! Regression for retiring the SQLite side store.
use super::*;
use crate::domain::{
    ContentKind,
    model::{EmbeddingMetric, ModelInfo},
};

#[test]
fn ordinary_index_store_never_creates_sqlite() {
    let root = tempfile::tempdir().expect("tempdir");
    let model = EmbeddingModelInfo::new(
        ModelInfo::new(
            "fixture",
            "graph",
            [ContentKind::Text, ContentKind::Code],
            32,
        )
        .expect("model"),
        3,
        EmbeddingMetric::Cosine,
        None,
        None,
    )
    .expect("embedding");
    let store = IndexStore::open(WorkspaceIndexStorageOptions::ReadWrite {
        storage_path: root.path().to_owned(),
        embeddings: vec![model],
    })
    .expect("open");
    assert!(!root.path().join("graph/graph.sqlite").exists());
    store.close().expect("close");
    assert!(!root.path().join("graph/graph.sqlite").exists());
}
