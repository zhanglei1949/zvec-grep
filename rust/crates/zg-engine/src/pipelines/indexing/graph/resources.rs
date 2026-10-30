//! Workspace-wide candidate admission. These are checkpoint limits, not a disk
//! quota: one native transaction/checkpoint may grow before the next check.
use crate::{
    EngineError, EngineResult, storage::graph_state, workspace::manifest::WorkspaceManifest,
};
use std::{
    fs,
    path::{Path, PathBuf},
};

#[derive(Clone, Copy)]
pub(super) struct CandidateBudget {
    pub max_candidates: u64,
    pub max_bytes: u64,
    pub max_candidate_bytes: u64,
    pub min_free_bytes: u64,
    pub start_reserve_bytes: u64,
    pub max_entries: u64,
}
impl Default for CandidateBudget {
    fn default() -> Self {
        Self {
            max_candidates: 8,
            max_bytes: 1024 * 1024 * 1024,
            max_candidate_bytes: 256 * 1024 * 1024,
            min_free_bytes: 256 * 1024 * 1024,
            start_reserve_bytes: 64 * 1024 * 1024,
            max_entries: 100_000,
        }
    }
}
#[derive(Default, Debug)]
struct Inventory {
    candidates: u64,
    bytes: u64,
    current_bytes: u64,
    entries: u64,
}
fn exhausted() -> EngineError {
    EngineError::invalid_argument(
        "graph candidate retention or disk budget exceeded; inspect/discard retired candidates",
    )
}
fn io(error: &std::io::Error) -> EngineError {
    EngineError::from_io("inspect graph candidate resources", error)
}
fn present(path: &Path) -> EngineResult<bool> {
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.is_dir() && !meta.file_type().is_symlink() => Ok(true),
        Ok(_) => Err(EngineError::storage_failure(
            "invalid graph candidate ownership path",
        )),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(io(&e)),
    }
}
fn add_entry(inventory: &mut Inventory, budget: CandidateBudget) -> EngineResult<()> {
    inventory.entries = inventory.entries.checked_add(1).ok_or_else(exhausted)?;
    if inventory.entries > budget.max_entries {
        return Err(exhausted());
    }
    Ok(())
}
fn inventory(home: &Path, current: &Path, budget: CandidateBudget) -> EngineResult<Inventory> {
    let mut inventory = Inventory::default();
    let generations = home.join("generations");
    if !present(&generations)? {
        return Ok(inventory);
    }
    for generation in fs::read_dir(generations).map_err(|e| io(&e))? {
        let generation = generation.map_err(|e| io(&e))?;
        add_entry(&mut inventory, budget)?;
        if !graph_state::valid_uuid(&generation.file_name().to_string_lossy())
            || !present(&generation.path())?
        {
            return Err(EngineError::storage_failure(
                "invalid graph generation ownership",
            ));
        }
        let graph = generation.path().join("graph");
        if !present(&graph)? {
            continue;
        }
        for container in ["builds", "trash", "discards"] {
            let root = graph.join(container);
            if !present(&root)? {
                continue;
            }
            let mut pending = vec![root.clone()];
            while let Some(directory) = pending.pop() {
                for entry in fs::read_dir(directory).map_err(|e| io(&e))? {
                    let entry = entry.map_err(|e| io(&e))?;
                    add_entry(&mut inventory, budget)?;
                    let path = entry.path();
                    let meta = fs::symlink_metadata(&path).map_err(|e| io(&e))?;
                    if meta.file_type().is_symlink() || (!meta.is_dir() && !meta.is_file()) {
                        return Err(EngineError::storage_failure(
                            "unexpected graph candidate file type",
                        ));
                    }
                    if meta.is_dir() {
                        if path.parent() == Some(root.as_path()) && container != "discards" {
                            if !graph_state::valid_uuid(&entry.file_name().to_string_lossy()) {
                                return Err(EngineError::storage_failure(
                                    "invalid graph candidate identity",
                                ));
                            }
                            inventory.candidates += 1;
                        }
                        pending.push(path);
                    } else {
                        inventory.bytes = inventory
                            .bytes
                            .checked_add(meta.len())
                            .ok_or_else(exhausted)?;
                        if path.starts_with(current) {
                            inventory.current_bytes = inventory
                                .current_bytes
                                .checked_add(meta.len())
                                .ok_or_else(exhausted)?;
                        }
                    }
                    if inventory.bytes > budget.max_bytes
                        || inventory.current_bytes > budget.max_candidate_bytes
                        || inventory.candidates > budget.max_candidates
                    {
                        return Err(exhausted());
                    }
                }
            }
        }
    }
    Ok(inventory)
}

fn available_bytes(path: &Path) -> EngineResult<u64> {
    let existing = path
        .ancestors()
        .find(|p| p.is_dir())
        .ok_or_else(exhausted)?;
    let canonical = existing.canonicalize().map_err(|e| io(&e))?;
    let disks = sysinfo::Disks::new_with_refreshed_list();
    // Device identity also handles macOS firmlinks, whose visible paths need
    // not start with the Data volume's actual mount point.
    #[cfg(unix)]
    let device = {
        use std::os::unix::fs::MetadataExt;
        fs::metadata(&canonical).map_err(|e| io(&e))?.dev()
    };
    disks
        .list()
        .iter()
        .filter(|disk| {
            #[cfg(unix)]
            {
                use std::os::unix::fs::MetadataExt;
                fs::metadata(disk.mount_point()).is_ok_and(|m| m.dev() == device)
            }
            #[cfg(not(unix))]
            {
                canonical.starts_with(disk.mount_point())
            }
        })
        .max_by_key(|disk| {
            if canonical.starts_with(disk.mount_point()) {
                disk.mount_point().components().count()
            } else {
                0
            }
        })
        .map(sysinfo::Disk::available_space)
        .ok_or_else(|| EngineError::storage_failure("cannot determine graph candidate disk space"))
}

pub(super) struct CandidateGuard {
    home: PathBuf,
    current: PathBuf,
    budget: CandidateBudget,
}
impl CandidateGuard {
    pub fn new(manifest: &WorkspaceManifest, id: &str) -> Self {
        Self {
            home: manifest.path.clone(),
            current: manifest.storage_home().join("graph/builds").join(id),
            budget: CandidateBudget::default(),
        }
    }
    pub fn check(&self, starting: bool) -> EngineResult<()> {
        self.check_with(starting, available_bytes)
    }
    fn check_with(
        &self,
        starting: bool,
        free: impl FnOnce(&Path) -> EngineResult<u64>,
    ) -> EngineResult<()> {
        let inventory = inventory(&self.home, &self.current, self.budget)?;
        let reserve = if starting {
            self.budget.start_reserve_bytes
        } else {
            0
        };
        if starting && inventory.candidates >= self.budget.max_candidates
            || free(&self.current)? < self.budget.min_free_bytes.saturating_add(reserve)
            || inventory.bytes.saturating_add(reserve) > self.budget.max_bytes
        {
            return Err(exhausted());
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "resources/tests.rs"]
mod tests;
