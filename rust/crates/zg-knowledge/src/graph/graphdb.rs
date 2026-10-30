use super::{
    GraphError, GraphErrorKind, GraphOutcome, GraphResult, QueryControl,
    schema::{self, MetadataRow},
};
use crate::{
    processing::{ProcessingContext, stable_digest},
    relations::bound::FactCatalog,
};
use ::graphdb::{AccessMode, Database, QueryOptions, StorageMode, Transaction, TransactionMode};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex, PoisonError,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

#[path = "codec.rs"]
mod codec;
#[path = "read.rs"]
mod read;
use codec::FactRows;

#[path = "import.rs"]
mod import;
pub use import::{ImportBudget, ImportProgress};

/// These limits apply per query plus aggregate fact validation. The caller
/// owns blocking-task admission. There are no detached tasks or connections.
#[derive(Clone, Debug)]
pub struct GraphLimits {
    pub max_rows: usize,
    pub max_intermediate_rows: usize,
    pub admission_timeout: Duration,
    pub control: Option<QueryControl>,
}
impl Default for GraphLimits {
    fn default() -> Self {
        Self {
            max_rows: 100_000,
            max_intermediate_rows: 1_000_000,
            admission_timeout: Duration::from_secs(5),
            control: None,
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoreCounts {
    pub nodes: BTreeMap<String, u64>,
    pub relations: BTreeMap<String, u64>,
}

/// One directory-owning database instance; sessions are per operation. Calls
/// must execute entirely in a host bounded blocking task, never across await.
pub struct GraphDbStore {
    database: Database,
    path: PathBuf,
    limits: GraphLimits,
    read_only: bool,
    write: Mutex<()>,
    unusable: Arc<AtomicBool>,
}
impl GraphDbStore {
    /// Create a new candidate with opt-in root/receipt tables. Existing fact
    /// candidates are never upgraded by this API or by `open()`.
    pub fn create_build(path: impl AsRef<Path>, limits: GraphLimits) -> GraphResult<Self> {
        let store = Self::create(path, limits)?;
        let mut connection = store.database.connect()?;
        for ddl in schema::build_statements() {
            connection
                .execute_with(&ddl, (), &store.options())
                .map_err(|e| store.control_error(e.into()))?;
            if connection.in_transaction() {
                return Err(GraphError::local(GraphErrorKind::Storage));
            }
        }
        connection.close()?;
        Ok(store)
    }

    /// Create a new, unselected candidate. Existing nonempty paths are refused.
    pub fn create(path: impl AsRef<Path>, limits: GraphLimits) -> GraphResult<Self> {
        let path = path.as_ref();
        if path.exists() {
            if !path.is_dir() || fs::read_dir(path)?.next().is_some() {
                return Err(GraphError::local(GraphErrorKind::Conflict));
            }
        } else {
            fs::create_dir(path)?;
        }
        let store = Self::open_native(path, false, limits)?;
        let mut connection = store.database.connect()?;
        for ddl in schema::statements() {
            connection
                .execute_with(&ddl, (), &store.options())
                .map_err(|e| store.control_error(e.into()))?;
            if connection.in_transaction() {
                return Err(GraphError::local(GraphErrorKind::Storage));
            }
        }
        connection.close()?;
        // No ready marker before a complete write. An interrupted candidate is
        // intentionally not openable and is never implicitly upgraded/repaired.
        Ok(store)
    }

    pub fn open(path: impl AsRef<Path>, read_only: bool, limits: GraphLimits) -> GraphResult<Self> {
        let path = path.as_ref();
        if !path.is_dir() || fs::read_dir(path)?.next().is_none() {
            return Err(GraphError::local(GraphErrorKind::IncompatibleFormat));
        }
        let store = Self::open_native(path, read_only, limits)?;
        store.with_read(|tx, options| {
            let rows = MetadataRow::read(tx, options)?;
            check_metadata(&rows)?;
            Ok(())
        })?;
        Ok(store)
    }

    fn open_native(path: &Path, read_only: bool, limits: GraphLimits) -> GraphResult<Self> {
        if limits.max_rows == 0 || limits.max_intermediate_rows == 0 {
            return Err(GraphError::local(GraphErrorKind::InvalidInput));
        }
        let database = Database::builder()
            .storage_mode(StorageMode::File)
            .transaction_mode(TransactionMode::Tp)
            .access_mode(if read_only {
                AccessMode::ReadOnly
            } else {
                AccessMode::ReadWrite
            })
            .open(path)?;
        Ok(Self {
            database,
            path: path.to_path_buf(),
            limits,
            read_only,
            write: Mutex::new(()),
            unusable: Arc::new(AtomicBool::new(false)),
        })
    }

    /// Borrow the existing native owner with fresh per-operation limits and
    /// cancellation. No second database open and no connection crosses await.
    /// A read session cannot write; quarantine is shared with the owner.
    pub fn read_session(&self, limits: GraphLimits) -> GraphResult<Self> {
        self.guard()?;
        if limits.max_rows == 0 || limits.max_intermediate_rows == 0 {
            return Err(GraphError::local(GraphErrorKind::InvalidInput));
        }
        Ok(Self {
            database: self.database.clone(),
            path: self.path.clone(),
            limits,
            read_only: true,
            write: Mutex::new(()),
            unusable: Arc::clone(&self.unusable),
        })
    }
    fn options(&self) -> QueryOptions {
        QueryOptions {
            max_results: self.limits.max_rows,
            max_intermediate_rows: self.limits.max_intermediate_rows,
            transaction_timeout: self.limits.admission_timeout,
            cancel: self.limits.control.as_ref().map(QueryControl::token),
            ..QueryOptions::default()
        }
    }
    fn guard(&self) -> GraphResult<()> {
        if self.unusable.load(Ordering::Acquire) {
            return Err(GraphError::local(GraphErrorKind::RecoveryRequired));
        }
        Ok(())
    }
    fn control_error(&self, error: GraphError) -> GraphError {
        match &self.limits.control {
            Some(c) => c.classify(error),
            None => error,
        }
    }
    fn finish<T>(&self, result: GraphResult<T>) -> GraphResult<T> {
        let result = result.map_err(|e| self.control_error(e));
        if result.as_ref().is_err_and(|e| {
            e.kind == GraphErrorKind::RecoveryRequired || e.outcome == Some(GraphOutcome::Unknown)
        }) {
            self.unusable.store(true, Ordering::Release);
        }
        result
    }
    fn session<T>(
        &self,
        write: bool,
        operation: impl FnOnce(&mut Transaction<'_>, &QueryOptions) -> GraphResult<T>,
    ) -> GraphResult<T> {
        self.guard()?;
        let mut connection = super::profile::measure("native.connect", || self.database.connect())
            .map_err(GraphError::from)?;
        let result = (|| {
            let mut tx = if write {
                connection.transaction()?
            } else {
                connection.transaction_read_only()?
            };
            match operation(&mut tx, &self.options()) {
                Ok(output) => {
                    super::profile::measure("native.commit", || tx.commit())?;
                    Ok(output)
                }
                Err(mut error) => {
                    if let Err(cleanup) = tx.rollback() {
                        error.cleanup = Some(Box::new(GraphError::from(cleanup)));
                        error.kind = GraphErrorKind::RecoveryRequired;
                    } else if error.outcome.is_none()
                        || error.outcome == Some(GraphOutcome::InTransaction)
                    {
                        error.outcome = Some(GraphOutcome::RolledBack);
                    }
                    Err(error)
                }
            }
        })();
        // Close failure cannot erase known commit success.
        let close = super::profile::measure("native.close", || connection.close())
            .map_err(GraphError::from);
        self.finish(match (result, close) {
            (Ok(output), Ok(())) => Ok(output),
            (Err(error), Ok(())) => Err(error),
            (Err(mut error), Err(cleanup)) => {
                error.cleanup = Some(Box::new(cleanup));
                error.kind = GraphErrorKind::RecoveryRequired;
                Err(error)
            }
            (Ok(_), Err(mut error)) => {
                error.outcome = Some(GraphOutcome::Committed);
                error.kind = GraphErrorKind::RecoveryRequired;
                Err(error)
            }
        })
    }
    pub(super) fn with_read<T>(
        &self,
        operation: impl FnOnce(&mut Transaction<'_>, &QueryOptions) -> GraphResult<T>,
    ) -> GraphResult<T> {
        self.session(false, operation)
    }
    pub(super) fn with_write<T>(
        &self,
        operation: impl FnOnce(&mut Transaction<'_>, &QueryOptions) -> GraphResult<T>,
    ) -> GraphResult<T> {
        if self.read_only {
            return Err(GraphError::local(GraphErrorKind::ReadOnly));
        }
        let _write = self.write.lock().unwrap_or_else(PoisonError::into_inner);
        self.session(true, operation)
    }

    /// Write once to an empty candidate, then confirm through a NEW READ ONLY
    /// transaction. No `GraphRoot`, Receipt, HEAD or published/search-ready claim.
    pub fn write_facts(
        &self,
        catalog: &FactCatalog,
        context: &ProcessingContext,
    ) -> GraphResult<StoreCounts> {
        catalog.validate(context)?;
        let rows = FactRows::encode(catalog)?;
        if rows.cost() > self.limits.max_rows {
            return Err(GraphError::local(GraphErrorKind::ResourceExhausted));
        }
        context.check_output(rows.cost())?;
        self.with_write(|tx, options| {
            if !schema::FileRow::read(tx, options)?.is_empty()
                || !MetadataRow::read(tx, options)?.is_empty()
            {
                return Err(GraphError::local(GraphErrorKind::Conflict));
            }
            rows.write(tx, options, context, true)?;
            MetadataRow {
                id: "facts".into(),
                schema_version: schema::SCHEMA_VERSION,
                schema_fingerprint: schema::fingerprint()?,
                facts_format_version: crate::relations::bound::FACTS_FORMAT_VERSION,
                config_fingerprint: catalog.config_fingerprint.clone(),
                selection_fingerprint: catalog.selection_fingerprint.clone(),
                content_digest: stable_digest(catalog)?,
                rows_digest: rows.digest()?,
            }
            .insert(tx, options)?;
            Ok(())
        })?;
        // Do not perform a post-commit cancellation check that masks a known
        // effect. G2.2 owns cancellation/result classification for publication.
        self.with_read(|tx, options| {
            let metadata = MetadataRow::read(tx, options)?;
            check_metadata(&metadata)?;
            let actual = FactRows::read(tx, options)?;
            if actual != rows {
                let mut error = GraphError::local(GraphErrorKind::Storage);
                error.outcome = Some(GraphOutcome::Committed);
                return Err(error);
            }
            actual.counts()
        })
        .map_err(|error| GraphError {
            // Confirmation is another transaction. Its rollback/cancellation
            // cannot turn the already committed candidate into not-applied.
            kind: error.kind,
            outcome: Some(GraphOutcome::Committed),
            stage: "confirm".into(),
            native_kind: None,
            transaction_code: None,
            mutation_code: None,
            cleanup: None,
            cause: Some(Box::new(error)),
        })
    }

    pub fn read_facts(&self, context: &ProcessingContext) -> GraphResult<FactCatalog> {
        let _stage = super::profile::stage("graph.read_facts");
        context.check()?;
        self.with_read(|tx, options| {
            let metadata = MetadataRow::read(tx, options)?;
            check_metadata(&metadata)?;
            let rows = FactRows::read(tx, options)?;
            context.check_output(rows.cost())?;
            let catalog = rows.decode(&metadata[0], context)?;
            if stable_digest(&catalog)? != metadata[0].content_digest
                || rows.digest()? != metadata[0].rows_digest
            {
                return Err(GraphError::local(GraphErrorKind::IncompatibleFormat));
            }
            Ok(catalog)
        })
    }
    pub fn counts(&self) -> GraphResult<StoreCounts> {
        self.with_read(|tx, options| FactRows::read(tx, options)?.counts())
    }
    pub fn checkpoint(&self) -> GraphResult<()> {
        if self.read_only {
            return Err(GraphError::local(GraphErrorKind::ReadOnly));
        }
        let _write = self.write.lock().unwrap_or_else(PoisonError::into_inner);
        self.guard()?;
        let mut connection = self.database.connect()?;
        let result = connection.checkpoint().map_err(GraphError::from);
        let close = connection.close().map_err(GraphError::from);
        self.finish(result.and(close))
    }
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }
}
fn check_metadata(rows: &[MetadataRow]) -> GraphResult<()> {
    if rows.len() != 1
        || rows[0].id != "facts"
        || rows[0].schema_version != schema::SCHEMA_VERSION
        || rows[0].schema_fingerprint != schema::fingerprint()?
        || rows[0].facts_format_version != crate::relations::bound::FACTS_FORMAT_VERSION
    {
        return Err(GraphError::local(GraphErrorKind::IncompatibleFormat));
    }
    Ok(())
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
