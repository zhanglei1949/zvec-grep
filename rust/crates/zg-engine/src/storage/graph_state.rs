//! Host-owned graph sidecars and fail-closed text mutation fencing.
//! This module is compiled even when graphdb is not enabled.
#[cfg(feature = "knowledge-graph-graphdb")]
mod build_record;
pub(crate) mod discard;
#[cfg(feature = "knowledge-graph-graphdb")]
pub(crate) use build_record::{BuildRecord, bound_matches, rules_fingerprint};

use crate::api::index::graph::GraphDomainResult;
use crate::{
    EngineError, EngineResult,
    domain::{FileIndexStatus, FileRecord},
    utils::{atomic_write, sync_directory},
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

pub(crate) fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}
pub(crate) fn fingerprint(value: &impl Serialize) -> EngineResult<String> {
    let value = serde_json::to_value(value).map_err(|_| invalid())?;
    let bytes = serde_json::to_vec(&value).map_err(|_| invalid())?;
    Ok(crate::utils::sha256_hex(&bytes))
}
fn invalid() -> EngineError {
    EngineError::storage_failure("invalid or unsupported graph metadata")
}
pub(crate) fn valid_digest(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit())
}
pub(crate) fn valid_uuid(s: &str) -> bool {
    uuid::Uuid::parse_str(s).is_ok()
}
pub(crate) fn read_record<T: DeserializeOwned>(path: &Path) -> EngineResult<Option<T>> {
    let bytes = match fs::read(path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(EngineError::from_io("read graph metadata", &e)),
    };
    let value: serde_json::Value = serde_json::from_slice(&bytes).map_err(|_| invalid())?;
    if value
        .get("formatVersion")
        .and_then(serde_json::Value::as_u64)
        != Some(1)
    {
        return Err(invalid());
    }
    serde_json::from_value(value)
        .map(Some)
        .map_err(|_| invalid())
}
pub(crate) fn write_record(path: &Path, value: &impl Serialize) -> EngineResult<()> {
    let parent = path.parent().ok_or_else(invalid)?;
    let mut missing = Vec::new();
    let mut ancestor = parent;
    while !ancestor
        .try_exists()
        .map_err(|e| EngineError::from_io("inspect graph directory", &e))?
    {
        missing.push(ancestor);
        ancestor = ancestor.parent().ok_or_else(invalid)?;
    }
    fs::create_dir_all(parent)
        .map_err(|e| EngineError::from_io("create graph metadata directory", &e))?;
    for p in missing.into_iter().rev() {
        sync_directory(p.parent().ok_or_else(invalid)?)?;
    }
    let bytes = serde_json::to_vec(value).map_err(|_| invalid())?;
    atomic_write(path, &bytes)
}
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum TextPhase {
    Stable,
    Mutating,
}
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct TextState {
    pub format_version: u32,
    pub state: TextPhase,
    pub text_revision: u64,
    pub operation_id: String,
    pub attempt: u64,
    pub catalog_digest: String,
    pub updated_epoch_ms: u64,
}
impl TextState {
    pub fn read(home: &Path) -> EngineResult<Option<Self>> {
        let state: Option<Self> = read_record(&home.join("graph/STATE.json"))?;
        if state.as_ref().is_some_and(|s| {
            !valid_uuid(&s.operation_id) || s.attempt == 0 || !valid_digest(&s.catalog_digest)
        }) {
            return Err(invalid());
        }
        Ok(state)
    }
    #[cfg(any(test, feature = "knowledge-graph-graphdb"))]
    pub fn baseline(home: &Path, digest: String) -> EngineResult<Self> {
        if let Some(state) = Self::read(home)? {
            if state.state != TextPhase::Stable || state.catalog_digest != digest {
                return Err(EngineError::resource_busy(
                    "graph text baseline requires reconciliation",
                ));
            }
            return Ok(state);
        }
        let state = Self {
            format_version: 1,
            state: TextPhase::Stable,
            text_revision: 0,
            operation_id: uuid::Uuid::new_v4().to_string(),
            attempt: 1,
            catalog_digest: digest,
            updated_epoch_ms: now(),
        };
        write_record(&home.join("graph/STATE.json"), &state)?;
        Ok(state)
    }
}
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct GraphHead {
    pub format_version: u32,
    pub graph_build_id: String,
    pub text_revision: u64,
    pub catalog_digest: String,
    pub contract_version: String,
    pub schema_fingerprint: String,
    pub policy_fingerprint: String,
    pub domains: BTreeMap<String, GraphDomainResult>,
}
impl GraphHead {
    pub fn read(home: &Path) -> EngineResult<Option<Self>> {
        let head: Option<Self> = read_record(&home.join("graph/HEAD.json"))?;
        if head.as_ref().is_some_and(|h| {
            !valid_uuid(&h.graph_build_id)
                || ![
                    &h.catalog_digest,
                    &h.schema_fingerprint,
                    &h.policy_fingerprint,
                ]
                .into_iter()
                .all(|s| valid_digest(s))
                || h.contract_version != "zg-relations-v1"
        }) {
            return Err(invalid());
        }
        Ok(head)
    }
    #[cfg(any(test, feature = "knowledge-graph-graphdb"))]
    pub fn select(
        home: &Path,
        expected_state: &TextState,
        base_head: Option<&str>,
        policy: &str,
        head: &Self,
    ) -> EngineResult<()> {
        if TextState::read(home)?.as_ref() != Some(expected_state)
            || expected_state.state != TextPhase::Stable
            || Self::read(home)?
                .as_ref()
                .map(|h| h.graph_build_id.as_str())
                != base_head
            || head.text_revision != expected_state.text_revision
            || head.catalog_digest != expected_state.catalog_digest
            || head.policy_fingerprint != policy
        {
            return Err(EngineError::resource_busy("graph publication base changed"));
        }
        let path = home.join("graph/HEAD.json");
        if let Err(e) = write_record(&path, head) {
            // Preserve ambiguous selection; callers must not delete this build.
            return Err(EngineError::storage_failure(format!(
                "graph selection requires recovery_required: {}",
                e.code()
            )));
        }
        if Self::read(home)?.as_ref() != Some(head) {
            return Err(invalid());
        }
        Ok(())
    }
    pub fn revoke(home: &Path) -> EngineResult<()> {
        Self::read(home)?;
        match fs::remove_file(home.join("graph/HEAD.json")) {
            Ok(()) => sync_directory(&home.join("graph")),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(EngineError::from_io(
                "revoke graph selection; recovery_required",
                &e,
            )),
        }
    }
}

/// Single canonical inventory algorithm shared by fencing and untrusted reads.
/// Timestamps and failure text are diagnostic, never source identity.
pub(crate) fn catalog_digest(files: &[FileRecord]) -> String {
    fn path_bytes(path: &Path) -> Vec<u8> {
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStrExt;
            let mut b = vec![1];
            b.extend(path.as_os_str().as_bytes());
            b
        }
        #[cfg(not(unix))]
        {
            let mut b = vec![2];
            b.extend(path.as_os_str().as_encoded_bytes());
            b
        }
    }
    let mut sorted = files
        .iter()
        .map(|f| (path_bytes(f.relative_path.as_ref()), f))
        .collect::<Vec<_>>();
    sorted.sort_by(|a, b| a.0.cmp(&b.0));
    let mut hash = Sha256::new();
    hash.update(b"zg-host-catalog-v1");
    let part = |h: &mut Sha256, b: &[u8]| {
        h.update((b.len() as u64).to_be_bytes());
        h.update(b);
    };
    for (path, file) in sorted {
        part(&mut hash, &path);
        hash.update(file.id.get().to_be_bytes());
        hash.update([match file.index_status {
            FileIndexStatus::NotIndexed => 0,
            FileIndexStatus::Indexed { .. } => 1,
            FileIndexStatus::Deleting => 2,
            FileIndexStatus::Failed { .. } => 3,
        }]);
        match &file.snapshot.content_hash {
            None => hash.update([0]),
            Some(h) => {
                hash.update([1]);
                part(&mut hash, h.as_bytes());
            }
        }
        hash.update(file.snapshot.size_bytes.to_be_bytes());
    }
    hex::encode(hash.finalize())
}

pub(super) struct TextFence {
    home: PathBuf,
    pending: Option<TextState>,
}
impl TextFence {
    pub fn new(home: PathBuf) -> Self {
        Self {
            home,
            pending: None,
        }
    }
    pub fn before_write(
        &mut self,
        snapshot: impl FnOnce() -> EngineResult<Vec<FileRecord>>,
    ) -> EngineResult<()> {
        if self.pending.is_some() {
            return Ok(());
        }
        let current = TextState::read(&self.home)?;
        let exists = |p: PathBuf| {
            p.try_exists()
                .map_err(|e| EngineError::from_io("inspect graph fence", &e))
        };
        if current.is_none()
            && !exists(self.home.join("graph/HEAD.json"))?
            && !exists(self.home.join("graph/builds"))?
        {
            return Ok(());
        }
        let files = snapshot()?;
        let revision = current
            .as_ref()
            .map_or(0, |s| s.text_revision)
            .checked_add(1)
            .ok_or_else(invalid)?;
        let state = TextState {
            format_version: 1,
            state: TextPhase::Mutating,
            text_revision: revision,
            operation_id: uuid::Uuid::new_v4().to_string(),
            attempt: 1,
            catalog_digest: catalog_digest(&files),
            updated_epoch_ms: now(),
        };
        write_record(&self.home.join("graph/STATE.json"), &state)?;
        self.pending = Some(state);
        Ok(())
    }
    pub fn checkpoint(
        &mut self,
        snapshot: impl FnOnce() -> EngineResult<Vec<FileRecord>>,
    ) -> EngineResult<()> {
        let Some(mut state) = self.pending.clone() else {
            return Ok(());
        };
        let files = snapshot()?;
        if files.iter().any(|f| {
            matches!(
                f.index_status,
                FileIndexStatus::NotIndexed | FileIndexStatus::Deleting
            )
        }) {
            return Ok(());
        }
        if TextState::read(&self.home)?.as_ref() != Some(&state) {
            return Err(invalid());
        }
        state.state = TextPhase::Stable;
        state.catalog_digest = catalog_digest(&files);
        state.updated_epoch_ms = now();
        write_record(&self.home.join("graph/STATE.json"), &state)?;
        self.pending = None;
        Ok(())
    }
}

/// Existing generation cleanup must not erase unresolved graph outcomes.
pub(crate) fn ensure_cleanup_safe(home: &Path) -> EngineResult<()> {
    discard::ensure_resolved(home)?;
    let entries = match fs::read_dir(home.join("graph/builds")) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(EngineError::from_io("inspect graph build ownership", &e)),
    };
    for entry in entries {
        let entry = entry.map_err(|e| EngineError::from_io("inspect graph build", &e))?;
        if !entry
            .file_type()
            .map_err(|e| EngineError::from_io("inspect graph build type", &e))?
            .is_dir()
        {
            return Err(invalid());
        }
        let state: Option<serde_json::Value> = read_record(&entry.path().join("attempt.json"))?;
        if state.as_ref().is_none_or(|s| {
            let phase = s["stage"].as_str().unwrap_or_default();
            let committed = s["commitEffect"].as_str().unwrap_or_default();
            let selection = s["selectionEffect"].as_str().unwrap_or_default();
            !matches!(
                phase,
                "registered" | "validated" | "confirmed" | "selected" | "failed"
            ) || !matches!(committed, "not_applied" | "committed")
                || !matches!(selection, "not_selected" | "selected")
        }) {
            return Err(EngineError::storage_failure(
                "graph candidate recovery_required; cleanup refused",
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
