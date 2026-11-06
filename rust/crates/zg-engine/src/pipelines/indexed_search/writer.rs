//! Queries may lease an active incremental writer until it retires for publication.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, Weak},
};
use tokio::sync::Notify;

use crate::{
    models::ModelRuntimeLease,
    storage::IndexStore,
    workspace::{lock::FileLock, manifest::WorkspaceManifest},
};

#[derive(Clone, Default)]
pub(crate) struct WriterContexts {
    entries: Arc<Mutex<HashMap<PathBuf, Weak<WriterSession>>>>,
}

pub(crate) struct WriterSession {
    // Borrowers retain all resources, including the home lock, if indexing is aborted.
    pub(crate) storage: IndexStore,
    pub(crate) models: Vec<ModelRuntimeLease>,
    pub(crate) manifest: WorkspaceManifest,
    readers: Mutex<Readers>,
    drained: Notify,
    _home_lock: Arc<FileLock>,
}

struct Readers {
    accepting: bool,
    active: usize,
}

pub(crate) struct WriterRegistration {
    contexts: WriterContexts,
    pub(crate) session: Arc<WriterSession>,
}

pub(crate) struct WriterLease {
    pub(crate) session: Arc<WriterSession>,
}

impl WriterSession {
    pub(crate) fn new(
        storage: IndexStore,
        models: Vec<ModelRuntimeLease>,
        manifest: WorkspaceManifest,
        home_lock: Arc<FileLock>,
    ) -> Self {
        Self {
            storage,
            models,
            manifest,
            readers: Mutex::new(Readers {
                accepting: true,
                active: 0,
            }),
            drained: Notify::new(),
            _home_lock: home_lock,
        }
    }

    async fn drain_readers(&self) {
        loop {
            let notified = self.drained.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            {
                let mut readers = self
                    .readers
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                readers.accepting = false;
                if readers.active == 0 {
                    return;
                }
            }
            notified.await;
        }
    }
}

impl WriterContexts {
    pub(crate) fn register(&self, session: WriterSession, borrowable: bool) -> WriterRegistration {
        let session = Arc::new(session);
        if borrowable {
            self.entries
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .insert(session.manifest.path.clone(), Arc::downgrade(&session));
        }
        WriterRegistration {
            contexts: self.clone(),
            session,
        }
    }

    pub(crate) fn borrow(&self, home: &Path, storage_home: &Path) -> Option<WriterLease> {
        let entries = self
            .entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let session = entries.get(home)?.upgrade()?;
        // Never expose a rebuild's unpublished generation or a different index.
        if session.manifest.storage_home() != storage_home {
            return None;
        }
        {
            let mut readers = session
                .readers
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if !readers.accepting {
                return None;
            }
            readers.active += 1;
        }
        Some(WriterLease { session })
    }
}

impl WriterRegistration {
    fn unpublish(&self) {
        let mut entries = self
            .contexts
            .entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if entries
            .get(&self.session.manifest.path)
            .is_some_and(|entry| Weak::ptr_eq(entry, &Arc::downgrade(&self.session)))
        {
            entries.remove(&self.session.manifest.path);
        }
    }

    pub(crate) async fn retire(&self) {
        self.unpublish();
        self.session.drain_readers().await;
    }
}

impl Drop for WriterRegistration {
    fn drop(&mut self) {
        self.unpublish();
    }
}

impl Drop for WriterLease {
    fn drop(&mut self) {
        let mut readers = self
            .session
            .readers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        readers.active -= 1;
        if readers.active == 0 {
            self.session.drained.notify_waiters();
        }
    }
}

#[async_trait::async_trait]
impl crate::pipelines::indexing::storage::IndexStorage for WriterSession {
    async fn prepare_write(&self) -> crate::EngineResult<()> {
        self.drain_readers().await;
        Ok(())
    }
    fn is_read_only(&self) -> bool {
        self.storage.is_read_only()
    }
    fn list_files(&self) -> crate::EngineResult<Vec<crate::domain::FileRecord>> {
        self.storage.list_files()
    }
    fn resolve_file_ids(
        &self,
        paths: &[PathBuf],
    ) -> crate::EngineResult<Vec<crate::domain::FileId>> {
        self.storage.resolve_file_ids(paths)
    }
    fn replace_file(
        &self,
        file: &crate::domain::FileRecord,
        entities: &[crate::domain::Entity],
        entries: &[crate::storage::types::IndexedFragment],
        graph: &crate::domain::FileGraphResult,
    ) -> crate::EngineResult<()> {
        self.storage
            .replace_file_with_graph(file, entities, entries, graph)
    }
    fn mark_file_failed(
        &self,
        file: &crate::domain::FileRecord,
        error: &str,
    ) -> crate::EngineResult<()> {
        self.storage.mark_file_failed(file, error)
    }
    fn delete_file(&self, id: crate::domain::FileId) -> crate::EngineResult<()> {
        self.storage.delete_file(id)
    }
    fn finalize_writes(&self) -> crate::EngineResult<()> {
        self.storage.finalize_writes()
    }
}
