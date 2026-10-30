//! Resident read handles, leased per query and retired before workspace writes.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex, MutexGuard, OnceLock, Weak,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use crate::{
    EngineError, EngineResult,
    workspace::lock::{FileLock, LockMode, acquire_read_write_lock, home_allows_cached_reads},
};

use super::{IndexStore, types::WorkspaceIndexStorageOptions};

#[cfg(feature = "knowledge-graph-graphdb")]
mod graph;

const REAP_INTERVAL: Duration = Duration::from_millis(50);
static CACHES: OnceLock<Mutex<Vec<Weak<CacheInner>>>> = OnceLock::new();

#[derive(Clone)]
pub(crate) struct ReadSessionCache {
    inner: Arc<CacheInner>,
}

struct CacheInner {
    state: Mutex<CacheState>,
    idle_timeout: Duration,
    closed: AtomicBool,
}

#[derive(Default)]
struct CacheState {
    entries: HashMap<PathBuf, Arc<SessionSlot>>,
}

#[derive(Default)]
struct SessionSlot {
    entry: Mutex<Option<CachedSession>>,
}

struct CachedSession {
    session: Arc<ReadSession>,
    last_used: Instant,
}

struct ReadSession {
    // Native handles must close before releasing the residency lock.
    storage: IndexStore,
    #[cfg(feature = "knowledge-graph-graphdb")]
    graph: Option<graph::GraphReadSession>,
    storage_home: PathBuf,
    graph_key: Option<String>,
    _residency: Option<FileLock>,
}

pub(crate) struct ReadSessionLease {
    session: Arc<ReadSession>,
    cache: Weak<SessionSlot>,
    cached: bool,
}

impl ReadSessionCache {
    pub(crate) fn new(idle_timeout: Duration) -> std::io::Result<Self> {
        let inner = Arc::new(CacheInner {
            state: Mutex::new(CacheState::default()),
            idle_timeout,
            closed: AtomicBool::new(false),
        });
        let weak = Arc::downgrade(&inner);
        // A native thread can release idle handles even when a synchronous writer
        // is waiting for another process's cache on a single-threaded executor.
        std::thread::Builder::new()
            .name("index-read-cache".into())
            .spawn(move || {
                while let Some(inner) = weak.upgrade() {
                    if !inner.reap(Instant::now()) {
                        break;
                    }
                    drop(inner);
                    std::thread::sleep(REAP_INTERVAL);
                }
            })?;
        let mut caches = lock(CACHES.get_or_init(Mutex::default));
        caches.retain(|cache| cache.strong_count() > 0);
        caches.push(Arc::downgrade(&inner));
        Ok(Self { inner })
    }

    /// The caller holds the workspace home read lock throughout the lease.
    pub(crate) fn acquire(
        &self,
        home: &Path,
        storage_home: &Path,
    ) -> EngineResult<ReadSessionLease> {
        self.acquire_with(home, storage_home, || {
            IndexStore::open(WorkspaceIndexStorageOptions::ReadOnly {
                storage_path: storage_home.to_path_buf(),
            })
        })
    }

    fn acquire_with(
        &self,
        home: &Path,
        storage_home: &Path,
        open: impl FnOnce() -> EngineResult<IndexStore>,
    ) -> EngineResult<ReadSessionLease> {
        self.acquire_session(home, storage_home, None, || {
            Ok(ReadSession {
                storage: open()?,
                #[cfg(feature = "knowledge-graph-graphdb")]
                graph: None,
                storage_home: storage_home.to_path_buf(),
                graph_key: None,
                _residency: None,
            })
        })
    }

    /// Borrow an immutable graph owner; the request validates eligibility with its own budget.
    #[cfg(feature = "knowledge-graph-graphdb")]
    pub(crate) fn acquire_graph_query(
        &self,
        manifest: &crate::workspace::manifest::WorkspaceManifest,
    ) -> EngineResult<ReadSessionLease> {
        let storage_home = manifest.storage_home();
        let key = graph::GraphReadSession::key(manifest)?;
        let lease = self.acquire_session(&manifest.path, &storage_home, Some(&key), || {
            let storage = IndexStore::open(WorkspaceIndexStorageOptions::ReadOnly {
                storage_path: storage_home.clone(),
            })?;
            let graph = graph::GraphReadSession::open(manifest, &key, &storage)?;
            Ok(ReadSession {
                storage,
                graph: Some(graph),
                storage_home: storage_home.clone(),
                graph_key: Some(key.clone()),
                _residency: None,
            })
        })?;
        Ok(lease)
    }

    #[cfg(all(test, feature = "knowledge-graph-graphdb"))]
    pub(crate) fn acquire_graph(
        &self,
        manifest: &crate::workspace::manifest::WorkspaceManifest,
    ) -> EngineResult<ReadSessionLease> {
        let lease = self.acquire_graph_query(manifest)?;
        let context = zg_knowledge::processing::ProcessingContext {
            max_input_bytes: 64 << 20,
            max_output_items: 1_000_000,
            deadline: Some(Instant::now() + Duration::from_secs(120)),
            cancellation: tokio_util::sync::CancellationToken::new(),
        };
        lease.graph_facts(
            manifest,
            zg_knowledge::graph::GraphLimits::default(),
            &context,
        )?;
        Ok(lease)
    }

    fn acquire_session(
        &self,
        home: &Path,
        storage_home: &Path,
        graph_key: Option<&str>,
        open: impl FnOnce() -> EngineResult<ReadSession>,
    ) -> EngineResult<ReadSessionLease> {
        let slot = {
            let mut state = lock(&self.inner.state);
            if self.inner.closed.load(Ordering::Acquire) {
                return Err(EngineError::resource_closed(
                    "index read cache has been closed",
                ));
            }
            Arc::clone(state.entries.entry(home.to_path_buf()).or_default())
        };
        // Only this workspace waits for native open/close; the map owns entries,
        // never native I/O. Concurrent misses for one workspace share this slot.
        let mut entry = lock(&slot.entry);
        if self.inner.closed.load(Ordering::Acquire) {
            return Err(EngineError::resource_closed(
                "index read cache has been closed",
            ));
        }
        if entry.as_ref().is_some_and(|entry| {
            entry.session.storage_home != storage_home
                || entry.session.graph_key.as_deref() != graph_key
        }) {
            entry.take();
        }
        if entry.is_none() {
            let residency = acquire_read_write_lock(
                &home.join("locks/read-cache"),
                LockMode::Read,
                "context.cache",
            )?;
            let session = ReadSession {
                _residency: Some(residency),
                ..open()?
            };
            *entry = Some(CachedSession {
                session: Arc::new(session),
                last_used: Instant::now(),
            });
        }
        let session = Arc::clone(&entry.as_ref().expect("initialized session").session);
        Ok(ReadSessionLease {
            session,
            cache: Arc::downgrade(&slot),
            cached: true,
        })
    }

    pub(crate) fn close(&self) {
        self.inner.closed.store(true, Ordering::Release);
        let entries = std::mem::take(&mut lock(&self.inner.state).entries);
        // In-flight queries retain their own Arc until they finish. Native close
        // runs after releasing the map lock, under the workspace's own slot lock.
        for slot in entries.into_values() {
            lock(&slot.entry).take();
        }
    }
}

impl std::fmt::Debug for ReadSessionCache {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ReadSessionCache")
            .field("entries", &lock(&self.inner.state).entries.len())
            .finish_non_exhaustive()
    }
}

impl CacheInner {
    fn reap(&self, now: Instant) -> bool {
        if self.closed.load(Ordering::Acquire) {
            return false;
        }
        let entries = lock(&self.state)
            .entries
            .iter()
            .map(|(home, slot)| (home.clone(), Arc::clone(slot)))
            .collect::<Vec<_>>();
        for (home, slot) in entries {
            // A cold open or another retirement must not stall maintenance of
            // other workspaces. It will be considered on the next pass.
            let Ok(mut entry) = slot.entry.try_lock() else {
                continue;
            };
            if let Some(cached) = entry.as_ref() {
                if Arc::strong_count(&cached.session) > 1 {
                    continue;
                }
                // A writer first owns the home lock, then waits for residency locks.
                // Releasing on any probe failure is conservative and never serves old data.
                if now.saturating_duration_since(cached.last_used) < self.idle_timeout
                    && home_allows_cached_reads(&home)
                {
                    continue;
                }
                entry.take();
            }
            drop(entry);
            let mut state = lock(&self.state);
            if Arc::strong_count(&slot) == 2
                && state
                    .entries
                    .get(&home)
                    .is_some_and(|current| Arc::ptr_eq(current, &slot))
            {
                state.entries.remove(&home);
            }
        }
        true
    }
}

impl ReadSessionLease {
    pub(crate) fn open(storage_home: &Path) -> EngineResult<Self> {
        Ok(Self {
            session: Arc::new(ReadSession {
                storage: IndexStore::open(WorkspaceIndexStorageOptions::ReadOnly {
                    storage_path: storage_home.to_path_buf(),
                })?,
                storage_home: storage_home.to_path_buf(),
                #[cfg(feature = "knowledge-graph-graphdb")]
                graph: None,
                graph_key: None,
                _residency: None,
            }),
            cache: Weak::new(),
            cached: false,
        })
    }

    pub(crate) fn storage(&self) -> &IndexStore {
        &self.session.storage
    }

    #[cfg(feature = "knowledge-graph-graphdb")]
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "G2 lease contract precedes G3 query entry points")
    )]
    pub(crate) fn graph_facts(
        &self,
        manifest: &crate::workspace::manifest::WorkspaceManifest,
        limits: zg_knowledge::graph::GraphLimits,
        context: &zg_knowledge::processing::ProcessingContext,
    ) -> EngineResult<zg_knowledge::relations::bound::FactCatalog> {
        if self.session.storage_home != manifest.storage_home() {
            return Err(EngineError::resource_busy("graph text generation changed"));
        }
        self.session
            .graph
            .as_ref()
            .ok_or_else(|| EngineError::resource_busy("this read lease has no selected graph"))?
            .read(manifest, self.storage(), limits, context)
    }

    #[cfg(feature = "knowledge-graph-graphdb")]
    pub(crate) fn open_graph(
        manifest: &crate::workspace::manifest::WorkspaceManifest,
    ) -> EngineResult<Self> {
        let storage_home = manifest.storage_home();
        let storage = IndexStore::open(WorkspaceIndexStorageOptions::ReadOnly {
            storage_path: storage_home.clone(),
        })?;
        let key = graph::GraphReadSession::key(manifest)?;
        let graph = graph::GraphReadSession::open(manifest, &key, &storage)?;
        Ok(Self {
            session: Arc::new(ReadSession {
                storage,
                graph: Some(graph),
                storage_home,
                graph_key: Some(key),
                _residency: None,
            }),
            cache: Weak::new(),
            cached: false,
        })
    }
    /// Used only while the caller retains the workspace admission lock.
    #[cfg(feature = "knowledge-graph-graphdb")]
    pub(crate) fn fork(&self) -> Self {
        Self {
            session: Arc::clone(&self.session),
            cache: self.cache.clone(),
            cached: self.cached,
        }
    }
    #[cfg(feature = "knowledge-graph-graphdb")]
    pub(crate) fn validate_graph(
        &self,
        manifest: &crate::workspace::manifest::WorkspaceManifest,
        limits: zg_knowledge::graph::GraphLimits,
        context: &zg_knowledge::processing::ProcessingContext,
    ) -> EngineResult<()> {
        self.session
            .graph
            .as_ref()
            .ok_or_else(|| EngineError::resource_busy("graph view missing"))?
            .validate_query(manifest, self.storage(), limits, context, true)
    }
    #[cfg(feature = "knowledge-graph-graphdb")]
    pub(crate) fn graph_reader(
        &self,
        limits: zg_knowledge::graph::GraphLimits,
    ) -> EngineResult<zg_knowledge::graph::GraphDbStore> {
        self.session
            .graph
            .as_ref()
            .ok_or_else(|| EngineError::resource_busy("graph view missing"))?
            .query_owner(limits)
    }
    #[cfg(feature = "knowledge-graph-graphdb")]
    pub(crate) fn graph_stamp(
        &self,
        manifest: &crate::workspace::manifest::WorkspaceManifest,
    ) -> EngineResult<zg_knowledge::query::ViewStamp> {
        self.session
            .graph
            .as_ref()
            .ok_or_else(|| EngineError::resource_busy("graph view missing"))?
            .query_stamp(manifest)
    }

    pub(crate) fn close(self) -> EngineResult<()> {
        if !self.cached {
            self.session.storage.close()?;
        }
        Ok(())
    }
}

impl Drop for ReadSessionLease {
    fn drop(&mut self) {
        let Some(cache) = self.cache.upgrade() else {
            return;
        };
        let mut entry = lock(&cache.entry);
        if let Some(entry) = entry.as_mut()
            && Arc::ptr_eq(&entry.session, &self.session)
        {
            entry.last_used = Instant::now();
        }
    }
}

/// Called after acquiring the exclusive home lock, before waiting for remote caches.
pub(crate) fn release_for_write(home: &Path) {
    let Some(caches) = CACHES.get() else {
        return;
    };
    let caches = lock(caches)
        .iter()
        .filter_map(Weak::upgrade)
        .collect::<Vec<_>>();
    for cache in caches {
        let slot = lock(&cache.state).entries.remove(home);
        if let Some(slot) = slot {
            lock(&slot.entry).take();
        }
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[cfg(test)]
mod tests;

#[cfg(all(test, feature = "knowledge-graph-graphdb"))]
mod graph_tests;
