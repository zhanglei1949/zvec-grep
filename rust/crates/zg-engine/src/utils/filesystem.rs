//! Engine error mapping for shared filesystem primitives.
use crate::{EngineError, EngineResult};
#[cfg(test)]
use std::fs::{self, File, Permissions};
use std::path::Path;

pub(crate) fn sync_directory(path: &Path) -> EngineResult<()> {
    zg_storage::filesystem::sync_directory(path)
        .map_err(|error| EngineError::from_io(error.context, &error.source))
}

pub(crate) fn atomic_write(path: &Path, bytes: &[u8]) -> EngineResult<()> {
    zg_storage::filesystem::atomic_write(path, bytes)
        .map_err(|error| EngineError::from_io(error.context, &error.source))
}

#[cfg(test)]
mod tests {
    use std::{collections::BTreeSet, ffi::OsString, io::Read};

    use super::*;

    fn assert_entries(directory: &Path, expected: &[&str]) {
        let entries: BTreeSet<_> = fs::read_dir(directory)
            .expect("read directory")
            .map(|entry| entry.expect("directory entry").file_name())
            .collect();
        assert_eq!(entries, expected.iter().map(OsString::from).collect());
    }

    fn assert_error(error: &EngineError, operation: &str, path: &Path) {
        assert!(error.message().contains(operation), "{error}");
        assert!(
            error.message().contains(&format!("'{}'", path.display())),
            "{error}"
        );
        assert!(error.origin().file.ends_with("utils/filesystem.rs"));
        assert!(error.origin().line > 0);
    }

    #[test]
    fn creates_and_replaces_english_and_chinese_files_without_residue() {
        let root = tempfile::tempdir().expect("temporary directory");
        for (directory, name, initial, replacement) in [
            (
                "english",
                "record.txt",
                "long original English contents",
                "替换内容",
            ),
            (
                "中文目录",
                "中文 记录.txt",
                "初始",
                "long replacement English contents",
            ),
        ] {
            let parent = root.path().join(directory);
            fs::create_dir(&parent).expect("prepare parent directory");
            let path = parent.join(name);
            for contents in [initial, replacement] {
                atomic_write(&path, contents.as_bytes()).expect("atomic write");
                assert_eq!(fs::read(&path).expect("read contents"), contents.as_bytes());
                assert_entries(&parent, &[name]);
                #[cfg(windows)]
                {
                    use std::os::windows::fs::MetadataExt;
                    const FILE_ATTRIBUTE_TEMPORARY: u32 = 0x100;
                    assert_eq!(
                        fs::metadata(&path)
                            .expect("file metadata")
                            .file_attributes()
                            & FILE_ATTRIBUTE_TEMPORARY,
                        0,
                    );
                }
            }
        }
    }

    #[test]
    fn creates_empty_files_and_replaces_contents_with_empty_bytes() {
        let root = tempfile::tempdir().expect("temporary directory");
        let path = root.path().join("record");
        for contents in [b"".as_slice(), b"previous contents", b""] {
            atomic_write(&path, contents).expect("atomic write");
            assert_eq!(fs::read(&path).expect("read contents"), contents);
            assert_entries(root.path(), &["record"]);
        }
    }

    #[test]
    fn rejects_invalid_paths_without_creating_parents() {
        let root = tempfile::tempdir().expect("temporary directory");
        let file = root.path().join("file");
        fs::write(&file, b"unchanged").expect("existing parent file");
        let missing = root.path().join("missing/record");
        let not_directory = file.join("record");
        let trailing_separator = file.join("");
        let trailing_dot = file.join(".");
        let missing_with_separator = root.path().join("new/");
        let missing_with_dot = root.path().join("new/.");
        let filesystem_root = root.path().ancestors().last().expect("filesystem root");
        for path in [
            Path::new(""),
            Path::new("."),
            Path::new(".."),
            filesystem_root,
            &missing,
            &not_directory,
            &trailing_separator,
            &trailing_dot,
            &missing_with_separator,
            &missing_with_dot,
        ] {
            let error = atomic_write(path, b"new").expect_err("invalid destination");
            assert_error(&error, "atomic file write failed", path);
            assert!(error.message().contains("destination not published"));
            assert_eq!(
                fs::read(&file).expect("unchanged parent file"),
                b"unchanged"
            );
            assert_entries(root.path(), &["file"]);
        }
    }

    #[test]
    fn rejects_directory_destinations() {
        let root = tempfile::tempdir().expect("temporary directory");
        let path = root.path().join("directory");
        fs::create_dir(&path).expect("existing directory");
        let error = atomic_write(&path, b"new").expect_err("directory destination");
        assert_error(
            &error,
            "atomic file write failed while reading permissions",
            &path,
        );
        assert!(path.is_dir());
        assert_entries(&path, &[]);
        assert_entries(root.path(), &["directory"]);
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlinks_without_changing_the_link_or_target() {
        use std::os::unix::fs::symlink;

        let root = tempfile::tempdir().expect("temporary directory");
        let target = root.path().join("target");
        let missing = root.path().join("missing");
        let link = root.path().join("link");
        fs::write(&target, b"unchanged").expect("symlink target");
        for destination in [&target, &missing] {
            symlink(destination, &link).expect("create symlink");
            let error = atomic_write(&link, b"new").expect_err("symlink destination");
            assert_error(
                &error,
                "atomic file write failed while reading permissions",
                &link,
            );
            assert_eq!(fs::read_link(&link).expect("unchanged link"), *destination);
            assert_eq!(fs::read(&target).expect("unchanged target"), b"unchanged");
            assert_entries(root.path(), &["link", "target"]);
            fs::remove_file(&link).expect("remove test symlink");
        }
    }

    #[cfg(unix)]
    #[test]
    fn creates_private_files_and_preserves_existing_modes() {
        use std::os::unix::fs::PermissionsExt;

        let root = tempfile::tempdir().expect("temporary directory");
        fs::set_permissions(root.path(), Permissions::from_mode(0o750)).expect("parent mode");
        let path = root.path().join("record");
        atomic_write(&path, b"initial").expect("create private file");
        let mode = fs::metadata(&path)
            .expect("file metadata")
            .permissions()
            .mode()
            & 0o7777;
        assert_eq!(mode & !0o600, 0); // umask may further restrict the new file.
        for mode in [0o640, 0o444] {
            fs::set_permissions(&path, Permissions::from_mode(mode)).expect("existing file mode");
            atomic_write(&path, b"replacement").expect("replace with preserved mode");
            assert_eq!(
                fs::metadata(&path)
                    .expect("file metadata")
                    .permissions()
                    .mode()
                    & 0o7777,
                mode
            );
            assert_eq!(
                fs::read(&path).expect("replacement contents"),
                b"replacement"
            );
            assert_eq!(
                fs::metadata(root.path())
                    .expect("parent metadata")
                    .permissions()
                    .mode()
                    & 0o7777,
                0o750
            );
            assert_entries(root.path(), &["record"]);
        }
    }

    #[cfg(windows)]
    #[test]
    fn preserves_windows_path_normalization() {
        let root = tempfile::tempdir().expect("temporary directory");
        let parent = dunce::simplified(root.path());
        let destination = parent.join("record");
        for name in ["record.", "record "] {
            let path = parent.join(name);
            for contents in [b"initial".as_slice(), b"replacement"] {
                atomic_write(&path, contents).expect("write normalized path");
                assert_eq!(fs::read(&path).expect("read original path"), contents);
                assert_eq!(
                    fs::read(&destination).expect("read normalized path"),
                    contents
                );
                assert_entries(root.path(), &["record"]);
            }
            fs::remove_file(&destination).expect("remove test file");
        }

        let literal = fs::canonicalize(root.path())
            .expect("verbatim parent")
            .join("literal.");
        atomic_write(&literal, b"literal name").expect("write verbatim path");
        assert_eq!(
            fs::read(&literal).expect("read verbatim path"),
            b"literal name"
        );
        assert_entries(root.path(), &["literal."]);
    }

    #[cfg(windows)]
    #[test]
    fn rejects_readonly_files_without_changing_contents_or_permissions() {
        let root = tempfile::tempdir().expect("temporary directory");
        let path = root.path().join("record");
        fs::write(&path, b"unchanged").expect("existing file");
        let original = fs::metadata(&path).expect("file metadata").permissions();
        let mut readonly = original.clone();
        readonly.set_readonly(true);
        fs::set_permissions(&path, readonly.clone()).expect("readonly file");
        let result = atomic_write(&path, b"new");
        let metadata = fs::metadata(&path);
        fs::set_permissions(&path, original).expect("restore permissions for cleanup");
        let error = result.expect_err("readonly destination");
        assert_eq!(error.code(), EngineError::PERMISSION_DENIED);
        assert_error(
            &error,
            "atomic file write failed while reading permissions",
            &path,
        );
        assert_eq!(metadata.expect("unchanged file").permissions(), readonly);
        assert_eq!(fs::read(&path).expect("unchanged contents"), b"unchanged");
        assert_entries(root.path(), &["record"]);
    }

    #[test]
    fn replacement_preserves_contents_visible_to_an_open_reader() {
        let root = tempfile::tempdir().expect("temporary directory");
        let path = root.path().join("record");
        fs::write(&path, b"original").expect("existing file");
        let mut reader = File::open(&path).expect("old reader");
        atomic_write(&path, b"replacement").expect("replace with reader open");
        let mut contents = Vec::new();
        reader
            .read_to_end(&mut contents)
            .expect("read old contents");
        assert_eq!(contents, b"original");
        assert_eq!(fs::read(&path).expect("read new contents"), b"replacement");
        assert_entries(root.path(), &["record"]);
    }

    #[test]
    fn syncs_existing_directories_and_reports_missing_ones() {
        let root = tempfile::tempdir().expect("temporary directory");
        sync_directory(root.path()).expect("sync existing directory");
        let file = root.path().join("record");
        fs::write(&file, b"unchanged").expect("ordinary file");
        let error = sync_directory(&file).expect_err("not a directory");
        assert_error(&error, "failed to sync directory", &file);
        assert_eq!(fs::read(&file).expect("unchanged file"), b"unchanged");
        let missing = root.path().join("missing");
        let error = sync_directory(&missing).expect_err("missing directory");
        assert_eq!(error.code(), EngineError::NOT_FOUND);
        assert_error(&error, "failed to sync directory", &missing);
        assert!(!error.message().contains("atomic file write"));
        assert_entries(root.path(), &["record"]);
    }
}
