//! Common filesystem utilities.

use std::{
    fs::{self, File, Permissions},
    io::{self, Write},
    path::Path,
};

use tempfile::{NamedTempFile, PersistError};

/// Detailed I/O failure; callers decide whether paths/messages are exposed.
#[derive(Debug, thiserror::Error)]
#[error("{context}: {source}")]
pub struct PersistenceError {
    pub context: String,
    pub source: io::Error,
}
impl PersistenceError {
    fn new(context: String, source: &io::Error) -> Self {
        // Retain kind and rendered cause for the caller's error mapping.
        Self {
            context,
            source: io::Error::new(source.kind(), source.to_string()),
        }
    }
}

pub fn sync_directory(path: &Path) -> Result<(), PersistenceError> {
    open_directory(nonempty_directory(path))
        .and_then(|directory| directory.sync_all())
        .map_err(|source| {
            PersistenceError::new(
                format!("failed to sync directory '{}'", path.display()),
                &source,
            )
        })
}

/// Prepare a complete file, atomically publish it, then sync its parent.
#[expect(
    clippy::too_many_lines,
    reason = "keep publication stages and error context together"
)]
pub fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), PersistenceError> {
    let failure = |operation: &str, source: &io::Error, cleanup: Option<io::Error>| {
        let cleanup = cleanup.map_or_else(String::new, |error| {
            format!(" (cleanup also failed: {error})")
        });
        PersistenceError::new(
            format!(
                "atomic file write failed {operation} for '{}'{cleanup}",
                path.display()
            ),
            source,
        )
    };
    // file_name() ignores trailing separators and '/.'; preserve the input's meaning.
    let input = path.as_os_str().as_encoded_bytes();
    let name = path
        .file_name()
        .filter(|name| input.ends_with(name.as_encoded_bytes()));
    // Normalize ordinary Windows names before canonicalize introduces a verbatim prefix.
    #[cfg(windows)]
    let normalized = std::path::absolute(path).map_err(|e| {
        failure(
            "while resolving destination path (destination not published)",
            &e,
            None,
        )
    })?;
    #[cfg(windows)]
    let (parent, name) = (
        normalized.parent(),
        name.and_then(|_| normalized.file_name()),
    );
    #[cfg(not(windows))]
    let parent = path.parent();
    let (Some(parent), Some(name)) = (parent, name) else {
        return Err(failure(
            "during path validation (destination not published)",
            &io::Error::new(
                io::ErrorKind::InvalidInput,
                "destination must end with a file name, not a separator or '.'",
            ),
            None,
        ));
    };
    let parent = fs::canonicalize(nonempty_directory(parent)).map_err(|e| {
        failure(
            "while resolving parent directory (destination not published)",
            &e,
            None,
        )
    })?;
    let destination = parent.join(name);
    let permissions = destination_permissions(&destination).map_err(|e| {
        failure(
            "while reading permissions (destination not published)",
            &e,
            None,
        )
    })?;
    let directory = open_directory(&parent).map_err(|e| {
        failure(
            "while opening parent directory (destination not published)",
            &e,
            None,
        )
    })?;
    directory.sync_all().map_err(|e| {
        failure(
            "during directory sync preflight (destination not published)",
            &e,
            None,
        )
    })?;

    let mut temporary = create_temporary(&parent).map_err(|e| {
        failure(
            "while creating temporary file (destination not published)",
            &e,
            None,
        )
    })?;
    if let Err(source) = prepare(temporary.as_file_mut(), bytes, permissions.as_ref()) {
        return Err(failure(
            "while preparing temporary file (destination not published)",
            &source,
            temporary.close().err(),
        ));
    }

    let file = publish(temporary, &destination, permissions.is_some()).map_err(|error| {
        failure(
            "during publication (outcome unknown)",
            &error.error,
            error.file.close().err(),
        )
    })?;
    // Windows no-clobber publication resets file attributes.
    #[cfg(windows)]
    file.sync_all().map_err(|e| {
        failure(
            "during file sync (destination published; durability unconfirmed)",
            &e,
            None,
        )
    })?;
    drop(file);
    directory.sync_all().map_err(|e| {
        failure(
            "during parent sync (destination published; durability unconfirmed)",
            &e,
            None,
        )
    })
}

fn prepare(file: &mut File, bytes: &[u8], permissions: Option<&Permissions>) -> io::Result<()> {
    file.write_all(bytes)?;
    #[cfg(unix)]
    if let Some(permissions) = permissions {
        file.set_permissions(permissions.clone())?;
        if file.metadata()?.permissions() != *permissions {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "filesystem did not preserve destination permissions",
            ));
        }
    }
    #[cfg(not(unix))]
    let _ = permissions; // Windows readonly destinations were already rejected.
    file.sync_all()
}

fn destination_permissions(path: &Path) -> io::Result<Option<Permissions>> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_file() => {
            let permissions = metadata.permissions();
            #[cfg(windows)]
            if permissions.readonly() {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "destination is readonly",
                ));
            }
            Ok(Some(permissions))
        }
        Ok(_) => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "destination must be a regular file, not a symlink or directory",
        )),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

fn create_temporary(parent: &Path) -> io::Result<NamedTempFile> {
    let mut builder = tempfile::Builder::new();
    builder.prefix(".atomic-");
    #[cfg(windows)]
    {
        // Keep tempfile's naming and cleanup, but omit FILE_ATTRIBUTE_TEMPORARY:
        // std::fs::rename preserves attributes when publishing the file.
        builder.make_in(parent, |path| File::create_new(path))
    }
    #[cfg(not(windows))]
    {
        builder.tempfile_in(parent) // Unix 0600, restricted by umask.
    }
}

fn publish(
    temporary: NamedTempFile,
    destination: &Path,
    overwrite: bool,
) -> Result<File, PersistError> {
    let result = if overwrite {
        // Unlike tempfile::persist, Windows std::fs::rename supports open readers.
        fs::rename(temporary.path(), destination)
    } else {
        #[cfg(any(target_os = "linux", target_os = "macos"))]
        {
            use rustix::fs::{CWD, RenameFlags, renameat_with};
            // Require a native rename; no hard-link/unlink fallback.
            renameat_with(
                CWD,
                temporary.path(),
                CWD,
                destination,
                RenameFlags::NOREPLACE,
            )
            .map_err(Into::into)
        }
        #[cfg(windows)]
        {
            return temporary.persist_noclobber(destination);
        }
        #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
        {
            Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "atomic creation is unsupported",
            ))
        }
    };
    if let Err(error) = result {
        return Err(PersistError {
            file: temporary,
            error,
        });
    }
    let mut temporary = temporary;
    temporary.disable_cleanup(true);
    Ok(temporary.into_file())
}

fn nonempty_directory(path: &Path) -> &Path {
    if path.as_os_str().is_empty() {
        Path::new(".")
    } else {
        path
    }
}

fn open_directory(path: &Path) -> io::Result<File> {
    // Check before opening so special files such as FIFOs cannot block the call.
    if !fs::metadata(path)?.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::NotADirectory,
            "path is not a directory",
        ));
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
        // Directory access for FlushFileBuffers; ordinary ACL checks still apply.
        fs::OpenOptions::new()
            .write(true)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
            .open(path)
    }
    #[cfg(not(windows))]
    {
        File::open(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn no_clobber_publication_preserves_a_competing_file() {
        let root = tempfile::tempdir().expect("temporary directory");
        let path = root.path().join("record");
        let mut temporary = create_temporary(root.path()).expect("temporary file");
        prepare(temporary.as_file_mut(), b"our contents", None).expect("prepare contents");
        fs::write(&path, b"competitor").expect("competing creator");
        let error = publish(temporary, &path, false).expect_err("must not replace competitor");
        error.file.close().expect("clean up unpublished file");
        assert_eq!(error.error.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(fs::read(&path).expect("competitor contents"), b"competitor");
        assert_eq!(
            fs::read_dir(root.path())
                .expect("directory entries")
                .count(),
            1
        );
    }
}
