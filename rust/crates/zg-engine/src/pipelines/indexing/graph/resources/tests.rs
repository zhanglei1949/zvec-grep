use super::*;

fn fixture() -> (tempfile::TempDir, CandidateGuard) {
    let dir = tempfile::tempdir().expect("workspace home");
    let current = dir
        .path()
        .join("generations")
        .join(uuid::Uuid::new_v4().to_string())
        .join("graph/builds")
        .join(uuid::Uuid::new_v4().to_string());
    fs::create_dir_all(&current).expect("candidate");
    let guard = CandidateGuard {
        home: dir.path().into(),
        current,
        budget: CandidateBudget {
            start_reserve_bytes: 0,
            min_free_bytes: 10,
            ..Default::default()
        },
    };
    (dir, guard)
}

#[test]
fn candidate_budget_counts_other_generations_unknown_candidates_and_trash() {
    let (dir, mut guard) = fixture();
    fs::write(guard.current.join("data"), [0_u8; 10]).expect("current bytes");
    let older = dir
        .path()
        .join("generations")
        .join(uuid::Uuid::new_v4().to_string())
        .join("graph/trash")
        .join(uuid::Uuid::new_v4().to_string());
    fs::create_dir_all(&older).expect("quarantine");
    fs::write(older.join("data"), [0_u8; 11]).expect("retained bytes");
    let measured = inventory(&guard.home, &guard.current, guard.budget).expect("inventory");
    assert_eq!(
        (measured.candidates, measured.bytes, measured.current_bytes),
        (2, 21, 10)
    );
    guard.budget.max_candidates = 2;
    guard
        .check_with(false, |_| Ok(100))
        .expect("existing two candidates");
    assert!(
        guard.check_with(true, |_| Ok(100)).is_err(),
        "new build must not exceed count"
    );
    guard.budget.max_bytes = 20;
    assert!(
        guard.check_with(false, |_| Ok(100)).is_err(),
        "quarantine counts against bytes"
    );
    assert!(older.exists(), "budget refusal never deletes evidence");
}

#[test]
fn candidate_budget_checks_current_growth_disk_reserve_and_metadata_bounds() {
    let (_dir, mut guard) = fixture();
    guard.budget.start_reserve_bytes = 20;
    assert!(guard.check_with(true, |_| Ok(29)).is_err());
    guard
        .check_with(true, |_| Ok(30))
        .expect("exact free-space floor");
    assert!(guard.check_with(false, |_| Ok(9)).is_err());
    fs::write(guard.current.join("data"), [0_u8; 12]).expect("growth");
    guard.budget.max_candidate_bytes = 11;
    assert!(guard.check_with(false, |_| Ok(100)).is_err());
    guard.budget.max_candidate_bytes = 100;
    guard.budget.max_entries = 1;
    assert!(guard.check_with(false, |_| Ok(100)).is_err());
}

#[test]
fn graph_disk_space_probe_reads_the_actual_filesystem() {
    let dir = tempfile::tempdir().expect("existing volume");
    assert!(available_bytes(&dir.path().join("not-created")).expect("actual free bytes") > 0);
}

#[cfg(unix)]
#[test]
fn candidate_budget_rejects_symlinked_generations() {
    use std::os::unix::fs::symlink;
    let (dir, guard) = fixture();
    let outside = tempfile::tempdir().expect("outside");
    symlink(
        outside.path(),
        dir.path()
            .join("generations")
            .join(uuid::Uuid::new_v4().to_string()),
    )
    .expect("symlink");
    assert!(guard.check_with(true, |_| Ok(u64::MAX)).is_err());
    assert_eq!(fs::read_dir(outside.path()).expect("untouched").count(), 0);
}
