//! Policy-free native operations. Callers retain schemas, ownership checks,
//! batch limits, error disclosure, and interpretation of per-item failures.
use zvec_rust::{
    Collection, CollectionOptions, CollectionSchema, DataType, Doc, FieldSchema, IndexParams,
};

pub fn create_collection(path: &str, schema: &CollectionSchema) -> zvec_rust::Result<Collection> {
    let options = CollectionOptions::new()?;
    Collection::create_and_open(path, schema, Some(&options))
}

pub fn open_collection(path: &str, read_only: bool) -> zvec_rust::Result<Collection> {
    let mut options = CollectionOptions::new()?;
    options.set_read_only(read_only)?;
    Collection::open(path, Some(&options))
}

#[derive(Clone, Copy, Debug)]
pub enum ScalarIndex {
    None,
    Exact,
    Wildcard,
}

pub fn add_scalar(
    schema: &mut CollectionSchema,
    name: &str,
    data_type: DataType,
    nullable: bool,
    index: ScalarIndex,
) -> zvec_rust::Result<()> {
    let mut field = FieldSchema::new(name, data_type, nullable, 0)?;
    let params = match index {
        ScalarIndex::None => None,
        ScalarIndex::Exact => Some(IndexParams::invert(true, false)?),
        ScalarIndex::Wildcard => Some(IndexParams::invert(false, true)?),
    };
    if let Some(params) = params {
        field.set_index_params(&params)?;
    }
    schema.add_field(&field)
}

pub fn fetch_docs(
    collection: &Collection,
    keys: &[String],
    batch_size: usize,
    include_vectors: bool,
) -> zvec_rust::Result<Vec<Doc>> {
    if batch_size == 0 {
        return Err(zvec_rust::Error {
            code: zvec_rust::ErrorCode::InvalidArgument,
            message: "fetch batch size must be positive".into(),
        });
    }
    let mut rows = Vec::new();
    for batch in keys.chunks(batch_size) {
        let keys = batch.iter().map(String::as_str).collect::<Vec<_>>();
        rows.extend(collection.fetch_with_options(&keys, None, include_vectors)?);
    }
    Ok(rows)
}

/// Validates the shape of a native response, including mixed-success results.
/// Whether failed rows abort an operation is the caller's policy.
pub fn write_counts_match(
    expected: usize,
    statuses: impl IntoIterator<Item = bool>,
    success_count: u64,
    error_count: u64,
) -> bool {
    let (mut count, mut successes) = (0u64, 0u64);
    for success in statuses {
        count += 1;
        successes += u64::from(success);
    }
    count == expected as u64 && successes == success_count && count - successes == error_count
}

#[cfg(test)]
mod tests {
    use super::write_counts_match;
    #[test]
    fn mixed_responses_are_valid_but_missing_or_inconsistent_counts_are_not() {
        assert!(write_counts_match(2, [true, false], 1, 1));
        assert!(!write_counts_match(2, [true], 1, 0));
        assert!(!write_counts_match(2, [true, false], 2, 0));
        assert!(!write_counts_match(2, [true, true], 1, 1));
    }
}
