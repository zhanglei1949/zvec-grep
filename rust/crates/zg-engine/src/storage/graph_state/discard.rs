//! Durable cleanup identity lives outside the directory being deleted. Native
//! proof and write/residency admission are the caller's responsibility.
#[cfg(any(test, feature = "knowledge-graph-graphdb"))]
use super::{GraphHead, write_record};
use super::{read_record, valid_uuid};
#[cfg(any(test, feature = "knowledge-graph-graphdb"))]
use crate::utils::sync_directory;
use crate::{EngineError, EngineResult};
use serde::{Deserialize, Serialize};
#[cfg(any(test, feature = "knowledge-graph-graphdb"))]
use std::path::PathBuf;
use std::{fs, path::Path};

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum Phase {
    Registered,
    Detached,
    Removed,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct DiscardRecord {
    format_version: u32,
    pub graph_build_id: String,
    pub operation_id: String,
    pub attempt: u64,
    pub candidate_committed: bool,
    phase: Phase,
}
fn invalid() -> EngineError {
    EngineError::storage_failure("graph discard recovery_required")
}
fn directory(path: &Path) -> EngineResult<bool> {
    match fs::symlink_metadata(path) {
        Ok(m) if m.is_dir() && !m.file_type().is_symlink() => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        _ => Err(invalid()),
    }
}
impl DiscardRecord {
    pub fn is_removed(&self) -> bool {
        self.phase == Phase::Removed
    }
    pub fn read(home: &Path, id: &str) -> EngineResult<Option<Self>> {
        if !valid_uuid(id) {
            return Err(invalid());
        }
        let records = home.join("graph/discards");
        if !directory(&records)? {
            return Ok(None);
        }
        let path = records.join(format!("{id}.json"));
        match fs::symlink_metadata(&path) {
            Ok(m) if m.is_file() && !m.file_type().is_symlink() => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            _ => return Err(invalid()),
        }
        let record: Option<Self> = read_record(&path)?;
        if record.as_ref().is_some_and(|r| {
            r.graph_build_id != id || !valid_uuid(&r.operation_id) || r.attempt == 0
        }) {
            return Err(invalid());
        }
        Ok(record)
    }
    #[cfg(any(test, feature = "knowledge-graph-graphdb"))]
    pub fn start(
        home: &Path,
        id: &str,
        operation: &str,
        attempt: u64,
        committed: bool,
    ) -> EngineResult<Self> {
        if !valid_uuid(id)
            || !valid_uuid(operation)
            || attempt == 0
            || Self::read(home, id)?.is_some()
        {
            return Err(invalid());
        }
        let record = Self {
            format_version: 1,
            graph_build_id: id.into(),
            operation_id: operation.into(),
            attempt,
            candidate_committed: committed,
            phase: Phase::Registered,
        };
        if GraphHead::read(home)?.is_some_and(|h| h.graph_build_id == id) {
            return Err(invalid());
        }
        record.save(home)?;
        Ok(record)
    }
    #[cfg(any(test, feature = "knowledge-graph-graphdb"))]
    fn record_path(&self, home: &Path) -> PathBuf {
        home.join("graph/discards")
            .join(format!("{}.json", self.graph_build_id))
    }
    #[cfg(any(test, feature = "knowledge-graph-graphdb"))]
    fn save(&self, home: &Path) -> EngineResult<()> {
        write_record(&self.record_path(home), self).map_err(|_| invalid())
    }
    #[cfg(any(test, feature = "knowledge-graph-graphdb"))]
    pub fn finish(self, home: &Path) -> EngineResult<Self> {
        self.finish_with(home, |path| fs::remove_dir_all(path))
            .map_err(|_| invalid())
    }
    #[cfg(any(test, feature = "knowledge-graph-graphdb"))]
    fn finish_with(
        mut self,
        home: &Path,
        remove: impl FnOnce(&Path) -> std::io::Result<()>,
    ) -> EngineResult<Self> {
        if GraphHead::read(home)?.is_some_and(|h| h.graph_build_id == self.graph_build_id) {
            return Err(invalid());
        }
        let builds = home.join("graph/builds");
        let trash = home.join("graph/trash");
        if !directory(&builds)? {
            return Err(invalid());
        }
        if !directory(&trash)? {
            fs::create_dir(&trash).map_err(|_| invalid())?;
            sync_directory(trash.parent().ok_or_else(invalid)?)?;
        }
        let source = builds.join(&self.graph_build_id);
        let detached = trash.join(&self.graph_build_id);
        let from = directory(&source)?;
        let to = directory(&detached)?;
        match self.phase {
            Phase::Registered => match (from, to) {
                (true, false) => fs::rename(&source, &detached).map_err(|_| invalid())?,
                (false, true) => {} // rename completed before its response/state was saved
                _ => return Err(invalid()),
            },
            Phase::Detached | Phase::Removed if from => return Err(invalid()),
            Phase::Removed if to => return Err(invalid()),
            _ => {}
        }
        sync_directory(&builds).map_err(|_| invalid())?;
        sync_directory(&trash).map_err(|_| invalid())?;
        if self.phase != Phase::Removed {
            self.phase = Phase::Detached;
            self.save(home)?;
            if directory(&detached)? {
                remove(&detached).map_err(|_| invalid())?;
            }
            sync_directory(&trash).map_err(|_| invalid())?;
            self.phase = Phase::Removed;
        }
        // Repeat a durable write even for an idempotent retry after a lost reply.
        self.save(home)?;
        Ok(self)
    }
}

pub(super) fn ensure_resolved(home: &Path) -> EngineResult<()> {
    let records = home.join("graph/discards");
    if directory(&records)? {
        for entry in fs::read_dir(records).map_err(|_| invalid())? {
            let entry = entry.map_err(|_| invalid())?;
            if !entry.file_type().map_err(|_| invalid())?.is_file() {
                return Err(invalid());
            }
            let id = entry
                .path()
                .file_stem()
                .and_then(|s| s.to_str())
                .ok_or_else(invalid)?
                .to_owned();
            let record = DiscardRecord::read(home, &id)?.ok_or_else(invalid)?;
            if !record.is_removed() || directory(&home.join("graph/builds").join(&id))? {
                return Err(invalid());
            }
        }
    }
    let trash = home.join("graph/trash");
    if directory(&trash)? && fs::read_dir(trash).map_err(|_| invalid())?.next().is_some() {
        return Err(invalid());
    }
    Ok(())
}

#[cfg(test)]
#[path = "discard/tests.rs"]
mod tests;
