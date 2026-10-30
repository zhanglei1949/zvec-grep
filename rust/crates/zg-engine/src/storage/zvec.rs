//! Shared zvec initialization and collection operations.
use std::{collections::HashMap, path::Path, sync::OnceLock};
use zvec_rust::{Collection, CollectionSchema, DataType, Doc};

use crate::{EngineError, EngineResult};

pub(super) const WRITE_BATCH: usize = 1024;
static INITIALIZED: OnceLock<Result<(), String>> = OnceLock::new();

pub(super) fn initialize() -> EngineResult<()> {
    match INITIALIZED.get_or_init(|| {
        let config = zvec_rust::ConfigBuilder::new()
            .num_threads(2)
            .memory_limit(512 * 1024 * 1024)
            .build();
        zvec_rust::initialize(Some(&config)).map_err(|error| error.to_string())
    }) {
        Ok(()) => Ok(()),
        Err(message) => Err(EngineError::storage_failure(format!(
            "cannot initialize zvec: {message}"
        ))),
    }
}

pub(super) fn open_collection(
    path: &Path,
    schema: &CollectionSchema,
    read_only: bool,
) -> EngineResult<Collection> {
    let text = native_path(path)?;
    if path.exists() {
        native(
            zg_storage::zvec::open_collection(text, read_only),
            &format!("open collection {}", path.display()),
        )
    } else if read_only {
        Err(EngineError::not_found(format!(
            "index collection does not exist: {}",
            path.display()
        )))
    } else {
        native(
            zg_storage::zvec::create_collection(text, schema),
            &format!("create collection {}", path.display()),
        )
    }
}

fn native_path(path: &Path) -> EngineResult<&str> {
    // zvec rejects the `?` in Windows verbatim prefixes. Simplify only when
    // the regular path identifies the same location; retain internal paths.
    #[cfg(windows)]
    let path = dunce::simplified(path);
    path.to_str().ok_or_else(|| {
        EngineError::invalid_argument(format!(
            "zvec storage path must be UTF-8: {}",
            path.display()
        ))
    })
}

pub(super) fn scalar(
    schema: &mut CollectionSchema,
    name: &str,
    data_type: DataType,
    nullable: bool,
    indexed: bool,
) -> EngineResult<()> {
    let index = if indexed {
        zg_storage::zvec::ScalarIndex::Exact
    } else {
        zg_storage::zvec::ScalarIndex::None
    };
    native(
        zg_storage::zvec::add_scalar(schema, name, data_type, nullable, index),
        "add scalar field",
    )
}

pub(super) fn wildcard_string(
    schema: &mut CollectionSchema,
    name: &str,
    nullable: bool,
) -> EngineResult<()> {
    native(
        zg_storage::zvec::add_scalar(
            schema,
            name,
            DataType::String,
            nullable,
            zg_storage::zvec::ScalarIndex::Wildcard,
        ),
        "add indexed path string",
    )
}

pub(super) fn fetch_map(
    collection: &Collection,
    keys: &[String],
) -> EngineResult<HashMap<String, Doc>> {
    let mut result = HashMap::with_capacity(keys.len());
    for doc in native(
        zg_storage::zvec::fetch_docs(collection, keys, WRITE_BATCH, false),
        "fetch stored records",
    )? {
        result.insert(doc_key(&doc)?.to_owned(), doc);
    }
    Ok(result)
}

pub(super) fn write_docs(
    collection: &Collection,
    docs: &[Doc],
    operation: &str,
) -> EngineResult<()> {
    for batch in docs.chunks(WRITE_BATCH) {
        let refs = batch.iter().collect::<Vec<_>>();
        let result = native(collection.upsert(&refs), operation)?;
        if !zg_storage::zvec::write_counts_match(
            batch.len(),
            result.results.iter().map(|s| s.success),
            result.success_count,
            result.error_count,
        ) || result.error_count != 0
        {
            let detail = result
                .results
                .iter()
                .find(|status| !status.success)
                .map_or_else(
                    || "incomplete write result".to_owned(),
                    |status| format!("{}: {}", status.code, status.message),
                );
            return Err(EngineError::storage_failure(format!(
                "{operation}: {detail}"
            )));
        }
    }
    Ok(())
}

pub(super) fn u32_field(doc: &Doc, name: &str) -> EngineResult<u32> {
    native(doc.get_u32(name), "read numeric field")?.ok_or_else(|| corrupt("missing numeric field"))
}

pub(super) fn doc_key(doc: &Doc) -> EngineResult<&str> {
    doc.get_pk()
        .ok_or_else(|| corrupt("stored record has no primary key"))
}

pub(super) fn string_field(doc: &Doc, field: &str) -> EngineResult<String> {
    native(doc.get_string(field), "read stored field")?
        .ok_or_else(|| corrupt(format!("stored field {field} is missing")))
}

pub(super) fn corrupt(message: impl std::fmt::Display) -> EngineError {
    EngineError::storage_failure(message.to_string())
}
#[track_caller]
pub(super) fn native<T>(result: zvec_rust::Result<T>, operation: &str) -> EngineResult<T> {
    result.map_err(|error| EngineError::storage_failure(format!("zvec {operation}: {error}")))
}
