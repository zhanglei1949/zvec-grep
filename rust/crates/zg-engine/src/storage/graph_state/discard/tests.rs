use super::*;
use crate::storage::graph_state::ensure_cleanup_safe;

fn fixture() -> (tempfile::TempDir, String, String) {
    let dir = tempfile::tempdir().expect("storage");
    let id = uuid::Uuid::new_v4().to_string();
    let op = uuid::Uuid::new_v4().to_string();
    let candidate = dir.path().join("graph/builds").join(&id);
    fs::create_dir_all(candidate.join("db")).expect("candidate");
    fs::write(candidate.join("db/one"), b"data").expect("file");
    fs::write(candidate.join("db/two"), b"more").expect("file");
    (dir, id, op)
}

#[test]
fn discard_resume_after_rename_and_lost_reply_is_idempotent() {
    let (dir, id, op) = fixture();
    DiscardRecord::start(dir.path(), &id, &op, 2, true).expect("durable cleanup identity");
    let trash = dir.path().join("graph/trash");
    fs::create_dir(&trash).expect("trash");
    fs::rename(dir.path().join("graph/builds").join(&id), trash.join(&id))
        .expect("rename completed, response lost");
    assert!(ensure_cleanup_safe(dir.path()).is_err());
    let pending = DiscardRecord::read(dir.path(), &id)
        .expect("read")
        .expect("record");
    assert!(!pending.is_removed());
    let removed = pending.finish(dir.path()).expect("resume physical state");
    assert!(removed.is_removed() && removed.candidate_committed);
    assert!(!trash.join(&id).exists());
    ensure_cleanup_safe(dir.path()).expect("known complete cleanup");
    DiscardRecord::read(dir.path(), &id)
        .expect("read")
        .expect("identity survives deletion")
        .finish(dir.path())
        .expect("retry after lost success response");
}

#[test]
fn discard_refuses_a_head_selected_after_registration() {
    let (dir, id, op) = fixture();
    let record = DiscardRecord::start(dir.path(), &id, &op, 1, true).expect("start");
    let head = GraphHead {
        format_version: 1,
        graph_build_id: id.clone(),
        text_revision: 0,
        catalog_digest: "a".repeat(64),
        contract_version: "zg-relations-v1".into(),
        schema_fingerprint: "b".repeat(64),
        policy_fingerprint: "c".repeat(64),
        domains: std::collections::BTreeMap::new(),
    };
    write_record(&dir.path().join("graph/HEAD.json"), &head).expect("conflicting publisher");
    assert!(record.finish(dir.path()).is_err());
    assert!(
        dir.path()
            .join("graph/builds")
            .join(&id)
            .join("db/one")
            .exists()
    );
}

#[cfg(unix)]
#[test]
fn discard_partial_filesystem_failure_keeps_external_identity_for_retry() {
    use std::os::unix::fs::PermissionsExt;
    let (dir, id, op) = fixture();
    let record = DiscardRecord::start(dir.path(), &id, &op, 1, false).expect("start");
    let result = record.finish_with(dir.path(), |path| {
        fs::remove_file(path.join("db/one"))?;
        fs::set_permissions(
            path.parent().expect("trash"),
            fs::Permissions::from_mode(0o500),
        )?;
        fs::remove_dir_all(path)
    });
    fs::set_permissions(
        dir.path().join("graph/trash"),
        fs::Permissions::from_mode(0o700),
    )
    .expect("restore permissions");
    assert!(
        result
            .expect_err("real removal refusal")
            .message()
            .contains("recovery_required")
    );
    assert!(
        ensure_cleanup_safe(dir.path()).is_err(),
        "default cleanup must preserve pending removal"
    );
    let record = DiscardRecord::read(dir.path(), &id)
        .expect("external receipt")
        .expect("present");
    assert_eq!(record.phase, Phase::Detached);
    assert_eq!(record.operation_id, op);
    assert!(!dir.path().join("graph/builds").join(&id).exists());
    assert!(
        record
            .finish(dir.path())
            .expect("retry partial deletion")
            .is_removed()
    );
    ensure_cleanup_safe(dir.path()).expect("resolved cleanup");
}

#[cfg(unix)]
#[test]
fn discard_state_write_failure_after_detach_preserves_registered_record() {
    use std::os::unix::fs::PermissionsExt;
    let (dir, id, op) = fixture();
    let record = DiscardRecord::start(dir.path(), &id, &op, 1, false).expect("start");
    let records = dir.path().join("graph/discards");
    fs::set_permissions(&records, fs::Permissions::from_mode(0o500)).expect("refuse record update");
    let result = record.finish(dir.path());
    fs::set_permissions(&records, fs::Permissions::from_mode(0o700)).expect("restore");
    assert!(result.is_err());
    let pending = DiscardRecord::read(dir.path(), &id)
        .expect("read")
        .expect("record");
    assert_eq!(pending.phase, Phase::Registered);
    assert!(dir.path().join("graph/trash").join(&id).exists());
    pending.finish(dir.path()).expect("rename checked on retry");
}

#[cfg(unix)]
#[test]
fn discard_rejects_symlinked_receipts_without_touching_target() {
    use std::os::unix::fs::symlink;
    let (dir, id, op) = fixture();
    let outside = tempfile::tempdir().expect("outside");
    symlink(outside.path(), dir.path().join("graph/discards")).expect("symlink");
    assert!(DiscardRecord::start(dir.path(), &id, &op, 1, false).is_err());
    assert_eq!(
        fs::read_dir(outside.path())
            .expect("outside unchanged")
            .count(),
        0
    );
}
