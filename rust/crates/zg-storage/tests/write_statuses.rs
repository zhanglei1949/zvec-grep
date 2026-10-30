//! Native mixed-success coverage retained when the independent KG index retired.
use zvec_rust::{
    CollectionSchema, DataType, Doc, FieldSchema, Fts, IndexParams, MetricType, SearchQuery,
};

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

fn schema() -> TestResult<CollectionSchema> {
    let mut schema = CollectionSchema::new("mixed_statuses")?;
    zg_storage::zvec::add_scalar(
        &mut schema,
        "revision",
        DataType::String,
        false,
        zg_storage::zvec::ScalarIndex::Exact,
    )?;
    let mut body = FieldSchema::new("body", DataType::String, false, 0)?;
    body.set_index_params(&IndexParams::fts(
        Some("standard"),
        Some(&["lowercase"]),
        None,
    )?)?;
    schema.add_field(&body)?;
    let mut vector = FieldSchema::new("embedding", DataType::VectorFp32, false, 4)?;
    vector.set_index_params(&IndexParams::flat(MetricType::Cosine)?)?;
    schema.add_field(&vector)?;
    Ok(schema)
}

fn row(id: &str, vector: bool) -> TestResult<Doc> {
    let mut doc = Doc::new()?;
    doc.try_set_pk(id)?;
    doc.add_string("body", "knowledge graph")?;
    doc.add_string("revision", "r1")?;
    if vector {
        doc.add_vector_f32("embedding", &[1.0, 0.0, 0.0, 0.0])?;
    }
    Ok(doc)
}

#[test]
fn native_mixed_statuses_repair_and_filtered_fts_vector_queries() -> TestResult {
    zvec_rust::initialize(Some(
        &zvec_rust::ConfigBuilder::new()
            .num_threads(2)
            .memory_limit(512 * 1024 * 1024),
    ))?;
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("index");
    let collection = zg_storage::zvec::create_collection(
        path.to_str().expect("UTF-8 temporary path"),
        &schema()?,
    )?;
    let first = row("first", true)?;
    let invalid = row("invalid", false)?;
    assert!(collection.upsert(&[&first, &invalid]).is_err());
    collection.upsert(&[&first])?;
    let second = row("second", true)?;
    let mixed = collection.insert(&[&first, &second])?;
    assert!(zg_storage::zvec::write_counts_match(
        2,
        mixed.results.iter().map(|s| s.success),
        mixed.success_count,
        mixed.error_count,
    ));
    assert!(!mixed.results[0].success && mixed.results[1].success);
    let third = row("third", true)?;
    let repaired = collection.upsert(&[&first, &second, &third])?;
    assert!(repaired.results.iter().all(|s| s.success));
    collection.flush()?;
    drop(collection);
    let reopened = zg_storage::zvec::open_collection(path.to_str().expect("UTF-8 path"), true)?;
    let mut fts = Fts::new()?;
    fts.set_query_string("knowledge")?;
    let mut text = SearchQuery::fts("body", &fts, 10)?;
    text.set_filter("revision = 'r1'")?;
    assert_eq!(reopened.query(&text)?.len(), 3);
    text.set_filter("revision = 'unpublished'")?;
    assert!(reopened.query(&text)?.is_empty());
    let mut vector = SearchQuery::new("embedding", &[1.0, 0.0, 0.0, 0.0], 10)?;
    vector.set_filter("revision = 'r1'")?;
    assert_eq!(reopened.query(&vector)?.len(), 3);
    Ok(())
}
