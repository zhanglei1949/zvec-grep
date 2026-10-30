use std::{collections::BTreeMap, fs, path::Path, sync::OnceLock, time::Duration};
use zvec_rust::{
    CollectionSchema, DataType, Doc, FieldSchema, IndexParams, MetricType, SearchQuery,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn initialize() {
    static INIT: OnceLock<()> = OnceLock::new();
    INIT.get_or_init(|| {
        zvec_rust::initialize(Some(
            &zvec_rust::ConfigBuilder::new()
                .num_threads(2)
                .memory_limit(512 * 1024 * 1024),
        ))
        .expect("initialize zvec");
    });
}

fn files(
    root: &Path,
) -> std::io::Result<BTreeMap<std::path::PathBuf, (String, std::time::SystemTime)>> {
    let mut out = BTreeMap::new();
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            out.extend(files(&entry.path())?);
        } else {
            out.insert(
                entry.path(),
                (
                    zg_storage::sha256_hex(&fs::read(entry.path())?),
                    entry.metadata()?.modified()?,
                ),
            );
        }
    }
    Ok(out)
}

fn verify_readonly(hnsw: bool) -> TestResult {
    initialize();
    for metric in [MetricType::Cosine, MetricType::Ip, MetricType::L2] {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("collection");
        let path = path.to_str().expect("UTF-8 temporary path");
        let mut schema = CollectionSchema::new("readonly_probe")?;
        let mut field = FieldSchema::new("embedding", DataType::VectorFp32, false, 4)?;
        let params = if hnsw {
            IndexParams::hnsw(metric, 16, 100)?
        } else {
            IndexParams::flat(metric)?
        };
        field.set_index_params(&params)?;
        schema.add_field(&field)?;
        let collection = zg_storage::zvec::create_collection(path, &schema)?;
        let mut doc = Doc::new()?;
        doc.try_set_pk("vector-1")?;
        doc.add_vector_f32("embedding", &[1.0, 0.0, 0.0, 0.0])?;
        assert_eq!(collection.insert(&[&doc])?.success_count, 1);
        collection.flush()?;
        collection.close()?;
        let baseline = files(dir.path())?;
        let first = zg_storage::zvec::open_collection(path, true)?;
        let second = zg_storage::zvec::open_collection(path, true)?;
        assert_eq!(files(dir.path())?, baseline, "open must not write");
        let query = SearchQuery::new("embedding", &[1.0, 0.0, 0.0, 0.0], 10)?;
        assert_eq!(first.query(&query)?.len(), 1);
        assert_eq!(second.fetch(&["vector-1"])?.len(), 1);
        assert!(first.upsert(&[&doc]).is_err());
        assert!(second.delete(&["vector-1"]).is_err());
        assert!(first.flush().is_err());
        assert_eq!(
            files(dir.path())?,
            baseline,
            "reads and rejected writes must not write"
        );
        // The bug changes seconds-resolution metadata on close. Waiting here
        // makes the old implementation fail rather than pass within one tick.
        std::thread::sleep(Duration::from_millis(1100));
        first.close()?;
        assert_eq!(
            files(dir.path())?,
            baseline,
            "explicit close must not write"
        );
        drop(second);
        assert_eq!(files(dir.path())?, baseline, "Drop must not write");
    }
    Ok(())
}

#[test]
fn flat_readonly_open_query_close_and_drop_preserve_files() -> TestResult {
    verify_readonly(false)
}

#[test]
fn hnsw_readonly_open_query_close_and_drop_preserve_files() -> TestResult {
    verify_readonly(true)
}
