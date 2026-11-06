//! Called only after a verified code graph has been selected under writer admission.
use crate::{EngineError, EngineResult};
use std::{fs, io::Read, path::Path};

pub(super) fn sqlite(home: &Path) -> EngineResult<()> {
    let directory = home.join("graph");
    let base = directory.join("graph.sqlite");
    if !base
        .try_exists()
        .map_err(|e| EngineError::from_io("inspect retired SQLite", &e))?
    {
        return Ok(());
    }
    let paths = [
        directory.join("graph.sqlite-shm"),
        directory.join("graph.sqlite-wal"),
        base.clone(),
    ];
    for path in &paths {
        match fs::symlink_metadata(path) {
            Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            _ => {
                return Err(EngineError::storage_failure(
                    "retired SQLite artifact is not a regular file",
                ));
            }
        }
    }
    let mut header = [0u8; 16];
    fs::File::open(base)
        .and_then(|mut file| file.read_exact(&mut header))
        .map_err(|e| EngineError::from_io("verify retired SQLite header", &e))?;
    if &header != b"SQLite format 3\0" {
        return Err(EngineError::storage_failure(
            "unknown legacy graph artifact retained",
        ));
    }
    for path in paths {
        match fs::remove_file(path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(EngineError::from_io("remove retired SQLite", &error)),
        }
    }
    crate::utils::sync_directory(&directory)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cleanup_removes_known_sqlite_files_and_preserves_unknown_data() {
        let directory = tempfile::tempdir().expect("tempdir");
        let graph = directory.path().join("graph");
        fs::create_dir(&graph).expect("graph");
        fs::write(graph.join("graph.sqlite"), b"unknown format").expect("unknown");
        assert!(sqlite(directory.path()).is_err());
        assert_eq!(
            fs::read(graph.join("graph.sqlite")).expect("retained"),
            b"unknown format"
        );
        fs::write(
            graph.join("graph.sqlite"),
            b"SQLite format 3\0old code relations",
        )
        .expect("old artifact");
        fs::write(graph.join("graph.sqlite-wal"), b"wal").expect("wal");
        sqlite(directory.path()).expect("retire");
        sqlite(directory.path()).expect("repeat");
        assert!(!graph.join("graph.sqlite").exists());
        assert!(!graph.join("graph.sqlite-wal").exists());
    }
}
