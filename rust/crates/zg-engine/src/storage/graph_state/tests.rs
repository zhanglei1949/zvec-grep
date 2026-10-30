use super::*;
use crate::{
    api::index::{
        IndexOptions,
        graph::{GraphOptions, GraphScope},
    },
    workspace::graph_policy::GraphPolicy,
};

fn head(state: &TextState, policy: &GraphPolicy) -> GraphHead {
    GraphHead {
        format_version: 1,
        graph_build_id: uuid::Uuid::new_v4().to_string(),
        text_revision: state.text_revision,
        catalog_digest: state.catalog_digest.clone(),
        contract_version: "zg-relations-v1".into(),
        schema_fingerprint: "a".repeat(64),
        policy_fingerprint: policy.policy_fingerprint.clone(),
        domains: BTreeMap::new(),
    }
}
#[test]
fn graph_policy_omission_explicit_off_and_unknown_format_preserve_intent() {
    assert!(
        serde_json::to_value(IndexOptions::default())
            .expect("json")
            .get("graph")
            .is_none()
    );
    let root = tempfile::tempdir().expect("home");
    let configuration = GraphOptions {
        scope: GraphScope::Documents,
        ..GraphOptions::default()
    };
    let enabled = GraphPolicy::save(root.path(), configuration).expect("persist intent");
    assert_eq!(
        GraphPolicy::read(root.path())
            .expect("read")
            .expect("policy"),
        enabled
    );
    let off = GraphPolicy::save(root.path(), GraphOptions::default()).expect("explicit Off");
    assert_ne!(off.policy_fingerprint, enabled.policy_fingerprint);
    let request = IndexOptions {
        graph: Some(GraphOptions::default()),
        ..IndexOptions::default()
    };
    assert_eq!(
        serde_json::from_value::<IndexOptions>(serde_json::to_value(&request).expect("json"))
            .expect("roundtrip")
            .graph,
        request.graph
    );
    let file = root.path().join("graph/POLICY.json");
    let mut unknown = serde_json::to_value(&off).expect("json");
    unknown["formatVersion"] = serde_json::json!(99);
    let bytes = serde_json::to_vec(&unknown).expect("json");
    fs::write(&file, &bytes).expect("unknown fixture");
    assert!(GraphPolicy::read(root.path()).is_err());
    assert!(GraphPolicy::save(root.path(), GraphOptions::default()).is_err());
    assert_eq!(fs::read(file).expect("preserved"), bytes);
}
#[test]
fn graph_head_compare_and_swap_checks_state_revision_and_previous_choice() {
    let root = tempfile::tempdir().expect("home");
    let policy = GraphPolicy::save(
        root.path(),
        GraphOptions {
            scope: GraphScope::Documents,
            ..GraphOptions::default()
        },
    )
    .expect("policy");
    let state = TextState::baseline(root.path(), catalog_digest(&[])).expect("state");
    let first = head(&state, &policy);
    GraphHead::select(
        root.path(),
        &state,
        None,
        &policy.policy_fingerprint,
        &first,
    )
    .expect("first selection");
    let bytes = fs::read(root.path().join("graph/HEAD.json")).expect("head");
    let next = head(&state, &policy);
    assert!(
        GraphHead::select(root.path(), &state, None, &policy.policy_fingerprint, &next).is_err()
    );
    assert_eq!(
        fs::read(root.path().join("graph/HEAD.json")).expect("unchanged"),
        bytes
    );
    let mut changed = state.clone();
    changed.text_revision += 1;
    write_record(&root.path().join("graph/STATE.json"), &changed).expect("concurrent text update");
    assert!(
        GraphHead::select(
            root.path(),
            &state,
            Some(&first.graph_build_id),
            &policy.policy_fingerprint,
            &next
        )
        .is_err()
    );
    assert_eq!(GraphHead::read(root.path()).expect("old head"), Some(first));
}
#[test]
fn graph_pending_outcomes_block_existing_generation_cleanup() {
    let dir = tempfile::tempdir().expect("storage");
    let build = dir
        .path()
        .join("graph/builds")
        .join(uuid::Uuid::new_v4().to_string());
    let record = |stage: &str, commit: &str, selection: &str| serde_json::json!({"formatVersion":1,"stage":stage,"commitEffect":commit,"selectionEffect":selection});
    for value in [
        record("commit_pending", "unknown", "not_selected"),
        record("select_pending", "committed", "unknown"),
        record("unknown", "unknown", "unknown"),
    ] {
        write_record(&build.join("attempt.json"), &value).expect("pending marker");
        assert!(ensure_cleanup_safe(dir.path()).is_err());
        assert!(build.exists());
    }
    write_record(
        &build.join("attempt.json"),
        &record("confirmed", "committed", "not_selected"),
    )
    .expect("known outcome");
    ensure_cleanup_safe(dir.path())
        .expect("known unselected candidate may be cleaned after leases close");
}

#[test]
fn catalog_digest_cost_probe() -> Result<(), Box<dyn std::error::Error>> {
    use crate::domain::{FileId, FileSnapshot, SourcePath};
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Input {
        id: u32,
        path: PathBuf,
        size_bytes: u64,
        content_hash: String,
    }
    let supplied = std::env::var_os("ZG_G2_CATALOG_RECORDS");
    let inputs: Vec<Input> = if let Some(path) = supplied {
        serde_json::from_slice(&fs::read(path)?)?
    } else {
        serde_json::from_value(serde_json::json!([
            {"id": 2, "path": "src/lib.rs", "size_bytes": 12, "content_hash": "b".repeat(64)},
            {"id": 1, "path": "README.md", "size_bytes": 8, "content_hash": "a".repeat(64)}
        ]))?
    };
    assert!(!inputs.is_empty());
    let files = inputs
        .into_iter()
        .map(|input| {
            Ok(FileRecord {
                id: FileId::new(input.id),
                relative_path: SourcePath::new(input.path)?,
                snapshot: FileSnapshot {
                    size_bytes: input.size_bytes,
                    modified_epoch_ms: None,
                    content_hash: Some(input.content_hash),
                },
                index_status: FileIndexStatus::Indexed {
                    indexed_epoch_ms: 0,
                    entity_count: 0,
                },
            })
        })
        .collect::<EngineResult<Vec<_>>>()?;
    for file in &files {
        file.validate()?;
    }
    let started = std::time::Instant::now();
    let digest = catalog_digest(&files);
    let duration_micros = started.elapsed().as_micros();
    assert_eq!(digest, catalog_digest(&files));
    let output = serde_json::json!({
        "records": files.len(),
        "digest": digest,
        "duration_micros": duration_micros,
        "repeat_equal": true,
    });
    if let Some(path) = std::env::var_os("ZG_G2_CATALOG_DIGEST_OUTPUT") {
        fs::write(path, serde_json::to_vec_pretty(&output)?)?;
    }
    println!("ZG_G2_CATALOG_DIGEST={output}");
    Ok(())
}

#[test]
fn graph_catalog_digest_ignores_mtime_but_keeps_native_paths_and_status() {
    use crate::domain::{FileId, FileSnapshot, SourcePath};
    let mut file = FileRecord {
        id: FileId::new(1),
        relative_path: SourcePath::new("中文.md").expect("path"),
        snapshot: FileSnapshot {
            size_bytes: 3,
            modified_epoch_ms: Some(1),
            content_hash: Some("a".repeat(64)),
        },
        index_status: FileIndexStatus::Indexed {
            indexed_epoch_ms: 1,
            entity_count: 1,
        },
    };
    let before = catalog_digest(&[file.clone()]);
    file.snapshot.modified_epoch_ms = Some(2);
    assert_eq!(before, catalog_digest(&[file.clone()]));
    file.index_status = FileIndexStatus::Failed {
        error: "test".into(),
    };
    assert_ne!(before, catalog_digest(&[file.clone()]));
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt;
        file.relative_path = SourcePath::new(PathBuf::from(std::ffi::OsString::from_vec(vec![
            0xff, b'.', b'm', b'd',
        ])))
        .expect("native path");
        let native = catalog_digest(&[file.clone()]);
        assert_ne!(native, catalog_digest(&[]));
        file.relative_path = SourcePath::new("�.md").expect("replacement character");
        assert_ne!(native, catalog_digest(&[file]));
    }
}
