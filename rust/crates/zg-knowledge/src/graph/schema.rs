//! Business graph with versioned construction auxiliaries private to the candidate.
//! Versioned attributes never contain source bodies or vectors.
use super::GraphResult;
use ::graphdb::{
    Batch, Column, ImportOptions, LogicalType, Params, QueryOptions, Schema, ToParam, Transaction,
};
use std::sync::Arc;

pub const SCHEMA_VERSION: u64 = 4;

type RawField = (
    &'static [u8],
    fn(&::graphdb::Row<'_>, usize, &mut Vec<u8>) -> GraphResult<()>,
);
trait CanonicalScalar: serde::Serialize {
    fn write_raw(row: &::graphdb::Row<'_>, index: usize, output: &mut Vec<u8>) -> GraphResult<()>;
}
fn write_scalar<T: serde::Serialize + ?Sized>(output: &mut Vec<u8>, value: &T) -> GraphResult<()> {
    serde_json::to_writer(output, value)
        .map_err(|_| super::GraphError::local(super::GraphErrorKind::InvalidInput))
}
impl CanonicalScalar for String {
    fn write_raw(row: &::graphdb::Row<'_>, index: usize, output: &mut Vec<u8>) -> GraphResult<()> {
        write_scalar(output, row.get_ref(index)?.as_str()?)
    }
}
impl CanonicalScalar for u64 {
    fn write_raw(row: &::graphdb::Row<'_>, index: usize, output: &mut Vec<u8>) -> GraphResult<()> {
        write_scalar(output, &row.try_get::<u64>(index)?)
    }
}
impl CanonicalScalar for bool {
    fn write_raw(row: &::graphdb::Row<'_>, index: usize, output: &mut Vec<u8>) -> GraphResult<()> {
        write_scalar(output, &row.try_get::<bool>(index)?)
    }
}
impl<T: CanonicalScalar> CanonicalScalar for Option<T> {
    fn write_raw(row: &::graphdb::Row<'_>, index: usize, output: &mut Vec<u8>) -> GraphResult<()> {
        if row.get_ref(index)?.is_null() {
            output.extend_from_slice(b"null");
            Ok(())
        } else {
            T::write_raw(row, index, output)
        }
    }
}

trait BatchColumn {
    fn logical_type() -> LogicalType;
    fn column<'a>(values: impl Iterator<Item = &'a Self>) -> GraphResult<Column>
    where
        Self: 'a;
}
impl BatchColumn for String {
    fn logical_type() -> LogicalType {
        LogicalType::String
    }
    fn column<'a>(values: impl Iterator<Item = &'a Self>) -> GraphResult<Column> {
        Ok(Column::strings(values.map(|v| Some(v.clone())).collect()))
    }
}
impl BatchColumn for Option<String> {
    fn logical_type() -> LogicalType {
        LogicalType::String
    }
    fn column<'a>(values: impl Iterator<Item = &'a Self>) -> GraphResult<Column> {
        Ok(Column::strings(values.cloned().collect()))
    }
}
impl BatchColumn for u64 {
    fn logical_type() -> LogicalType {
        LogicalType::UInt64
    }
    fn column<'a>(values: impl Iterator<Item = &'a Self>) -> GraphResult<Column> {
        Ok(values.copied().collect::<Vec<_>>().into())
    }
}
impl BatchColumn for Option<u64> {
    fn logical_type() -> LogicalType {
        LogicalType::UInt64
    }
    fn column<'a>(values: impl Iterator<Item = &'a Self>) -> GraphResult<Column> {
        let values = values.copied().collect::<Vec<_>>();
        Ok(Column::from(
            values
                .iter()
                .map(|v| v.unwrap_or_default())
                .collect::<Vec<_>>(),
        )
        .with_validity(values.iter().map(|v| u8::from(v.is_some())).collect())?)
    }
}
impl BatchColumn for bool {
    fn logical_type() -> LogicalType {
        LogicalType::Bool
    }
    fn column<'a>(values: impl Iterator<Item = &'a Self>) -> GraphResult<Column> {
        Ok(Column::bools(values.map(|v| u8::from(*v)).collect())?)
    }
}
impl BatchColumn for Option<bool> {
    fn logical_type() -> LogicalType {
        LogicalType::Bool
    }
    fn column<'a>(values: impl Iterator<Item = &'a Self>) -> GraphResult<Column> {
        let values = values.copied().collect::<Vec<_>>();
        Ok(Column::bools(
            values
                .iter()
                .map(|v| u8::from(v.unwrap_or_default()))
                .collect(),
        )?
        .with_validity(values.iter().map(|v| u8::from(v.is_some())).collect())?)
    }
}

// Resolve the fixed row order at compile time. Raw field writers borrow the
// native scalar window rather than copying strings into a second owned row.
const fn ordered_indices<const N: usize>(fields: [&'static str; N]) -> [usize; N] {
    const fn less(a: &str, b: &str) -> bool {
        let (a, b) = (a.as_bytes(), b.as_bytes());
        let mut i = 0;
        while i < a.len() && i < b.len() {
            if a[i] != b[i] {
                return a[i] < b[i];
            }
            i += 1;
        }
        a.len() < b.len()
    }
    let mut indices = [0; N];
    let mut i = 0;
    while i < N {
        indices[i] = i;
        i += 1;
    }
    i = 1;
    while i < N {
        let mut j = i;
        while j > 0 && less(fields[indices[j]], fields[indices[j - 1]]) {
            let old = indices[j - 1];
            indices[j - 1] = indices[j];
            indices[j] = old;
            j -= 1;
        }
        i += 1;
    }
    indices
}

// Domain row declarations generate parameterized statements, NOT a backend
// Value/Rows port. All table/column names are static and caller-independent.
macro_rules! table {
    ($name:ident, $table:literal, {$($field:ident : $ty:ty => $ddl:literal),+ $(,)?}) => {
        #[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
        pub(super) struct $name { $(pub $field: $ty,)+ }
        impl $name {
            pub const NAME: &'static str = $table;
            #[cfg(test)]
            #[allow(dead_code)] // Typed oracle also covers singleton table declarations.
            pub fn write_canonical(&self, output: &mut Vec<u8>) -> GraphResult<()> {
                const FIELDS: &[&str] = &[$(stringify!($field)),+];
                const ORDER: &[usize] = &ordered_indices([$(stringify!($field)),+]);
                output.push(b'{');
                for (i, index) in ORDER.iter().enumerate() {
                    let field = FIELDS[*index];
                    if i != 0 { output.push(b','); }
                    match field {
                        $(stringify!($field) => {
                            output.extend_from_slice(concat!("\"", stringify!($field), "\":").as_bytes());
                            write_scalar(output, &self.$field)?;
                        },)+
                        _ => unreachable!("fixed schema field"),
                    }
                }
                output.push(b'}');
                Ok(())
            }
            #[allow(dead_code)] // Singleton tables retain their existing proof encoding.
            pub fn write_raw(row: &::graphdb::Row<'_>, output: &mut Vec<u8>) -> GraphResult<()> {
                const ORDER: &[usize] = &ordered_indices([$(stringify!($field)),+]);
                const FIELDS: &[RawField] = &[$((concat!("\"", stringify!($field), "\":").as_bytes(), <$ty as CanonicalScalar>::write_raw)),+];
                output.push(b'{');
                for (i, index) in ORDER.iter().enumerate() {
                    if i != 0 { output.push(b','); }
                    let (prefix, writer) = FIELDS[*index];
                    output.extend_from_slice(prefix);
                    writer(row, *index, output)?;
                }
                output.push(b'}');
                Ok(())
            }
            #[allow(dead_code)] // Singleton publication tables use direct reads.
            pub fn scan<'a>(tx: &'a mut Transaction<'_>, options: &QueryOptions) -> GraphResult<::graphdb::OrderedScan<'a>> {
                Ok(tx.scan_nodes(Self::NAME, &[$(stringify!($field)),+], options)?)
            }
            pub fn ddl() -> String {
                let fields = [$(concat!(stringify!($field), " ", $ddl)),+];
                format!("CREATE NODE TABLE {}({} PRIMARY KEY{})", Self::NAME, fields[0], fields[1..].iter().map(|f| format!(",{f}")).collect::<String>())
            }
            pub fn insert(&self, tx: &mut Transaction<'_>, options: &QueryOptions) -> GraphResult<()> {
                let fields = [$(stringify!($field)),+];
                let query = format!("CREATE (:{} {{{}}})", Self::NAME, fields.iter().map(|f| format!("{f}: ${f}")).collect::<Vec<_>>().join(","));
                tx.execute_with(&query, Params::named(vec![$((stringify!($field), &self.$field as &dyn ToParam)),+]), options)?;
                Ok(())
            }
            #[allow(dead_code)] // Publication tables keep using parameterized inserts.
            pub fn import(rows: &[&Self], tx: &mut Transaction<'_>, options: &QueryOptions) -> GraphResult<()> {
                if rows.is_empty() { return Ok(()); }
                let schema = Arc::new(Schema::try_new(vec![$((stringify!($field).into(), <$ty as BatchColumn>::logical_type())),+])?);
                for chunk in rows.chunks(65_536) {
                    let batch = Batch::try_new(
                        schema.clone(),
                        chunk.len(),
                        vec![$(BatchColumn::column(chunk.iter().map(|row| &row.$field))?),+],
                    )?;
                    let summary = tx.import_batches(Self::NAME, batch.into_source(), ImportOptions {
                        query: options.clone(),
                        ..ImportOptions::default()
                    })?;
                    if summary.imported_rows != chunk.len() as u64 {
                        return Err(super::GraphError::local(super::GraphErrorKind::Storage));
                    }
                }
                Ok(())
            }
            #[allow(dead_code)] // Shared generator also covers singleton publication tables.
            pub fn page(tx: &mut Transaction<'_>, options: &QueryOptions, after: &str) -> GraphResult<Vec<Self>> {
                let fields = [$(stringify!($field)),+];
                let query = format!("MATCH (n:{}) WHERE n.{} > $after RETURN {} ORDER BY n.{} LIMIT 256", Self::NAME, fields[0], fields.iter().map(|f| format!("n.{f} AS {f}")).collect::<Vec<_>>().join(","), fields[0]);
                let result = tx.query_with(&query, ::graphdb::params!{"after"=>after}, options)?;
                let mut rows = result.rows(); let mut output = Vec::new();
                while let Some(row) = rows.next()? { output.push(Self { $($field: row.try_get(stringify!($field))?,)+ }); }
                Ok(output)
            }
            #[allow(dead_code)] // Only entity and source tables use selected reads.
            pub fn select(tx: &mut Transaction<'_>, options: &QueryOptions, clause: &str, parameters: Params<'_>, offset:u64) -> GraphResult<Vec<Self>> {
                let fields = [$(stringify!($field)),+];
                // The pinned binding does not bind parameters used only by SKIP.
                // This numeric offset is internal; all source inputs remain parameters.
                let query = format!("MATCH (n:{}) {} RETURN {} ORDER BY n.{} SKIP {offset} LIMIT 256", Self::NAME, clause, fields.iter().map(|f| format!("n.{f} AS {f}")).collect::<Vec<_>>().join(","), fields[0]);
                let result = tx.query_with(&query, parameters, options)?;
                let mut rows = result.rows(); let mut output = Vec::new();
                while let Some(row) = rows.next()? { output.push(Self { $($field: row.try_get(stringify!($field))?,)+ }); }
                Ok(output)
            }
            pub fn read(tx: &mut Transaction<'_>, options: &QueryOptions) -> GraphResult<Vec<Self>> {
                let fields = [$(stringify!($field)),+];
                let query = format!("MATCH (n:{}) RETURN {}", Self::NAME, fields.iter().map(|f| format!("n.{f} AS {f}")).collect::<Vec<_>>().join(","));
                let result = tx.query_with(&query, (), options)?;
                let mut rows = result.rows();
                let mut output = Vec::new();
                while let Some(row) = rows.next()? { output.push(Self { $($field: row.try_get(stringify!($field))?,)+ }); }
                Ok(output)
            }
        }
    };
    (@key $first:ident $(,$rest:ident)*) => {stringify!($first)};
}

table!(FileRow, "KgFile", {
    path: String => "STRING", file_id: u64 => "UINT64", index_status: String => "STRING", content_hash: Option<String> => "STRING",
    size_bytes: u64 => "UINT64", text_revision: u64 => "UINT64", storage_generation: String => "STRING",
    entity_count: u64 => "UINT64", entity_ids_digest: String => "STRING", facts_present: bool => "BOOL",
    language: Option<String> => "STRING", raw_digest: Option<String> => "STRING", decoded_digest: Option<String> => "STRING",
    namespace: Option<String> => "STRING", module_path: Option<String> => "STRING", is_package: Option<bool> => "BOOL",
    rules_fingerprint: Option<String> => "STRING", complete: Option<bool> => "BOOL", facts_format_version: u64 => "UINT64", text_map: String => "STRING", text_binding_digest: Option<String> => "STRING"
});
table!(NodeRow, "KgEntity", {
    id: String => "STRING", file_path: String => "STRING", ordinal: u64 => "UINT64", domain: String => "STRING", kind: String => "STRING", entity_kind: String => "STRING", name: String => "STRING",
    qualified_name: String => "STRING", qualified_label: String => "STRING", parent_id: Option<String> => "STRING", path_addressable: bool => "BOOL", callable: bool => "BOOL",
    span_start: u64 => "UINT64", span_end: u64 => "UINT64", entity_ids: String => "STRING"
});
table!(ScopeRow, "KgScope", { id: String => "STRING", file_path: String => "STRING", ordinal: u64 => "UINT64", kind: String => "STRING", owner_id: String => "STRING" });
table!(BindingRow, "KgBinding", {
    id: String => "STRING", file_path: String => "STRING", ordinal: u64 => "UINT64", scope_ordinal: u64 => "UINT64", name: String => "STRING",
    target_kind: String => "STRING", target_node_id: Option<String> => "STRING", target_import_path: Option<String> => "STRING", target_scope: Option<u64> => "UINT64", visible_from: u64 => "UINT64", visibility: String => "STRING"
});
table!(ReferenceRow, "KgUnresolvedRef", {
    id: String => "STRING", file_path: String => "STRING", ordinal: u64 => "UINT64", owner_id: String => "STRING", scope_ordinal: u64 => "UINT64",
    kind: String => "STRING", status: Option<String> => "STRING", reason: Option<String> => "STRING", span_start: u64 => "UINT64", span_end: u64 => "UINT64",
    lookup: String => "STRING", entity_ids: String => "STRING"
});
table!(AnchorRow, "KgAnchor", { id: String => "STRING", file_path: String => "STRING", ordinal: u64 => "UINT64", slug: String => "STRING", node_id: String => "STRING", discriminator: u64 => "UINT64" });
table!(DiagnosticRow, "KgDiagnostic", { id: String => "STRING", file_path: String => "STRING", ordinal: u64 => "UINT64", code: String => "STRING", span_start: Option<u64> => "UINT64", span_end: Option<u64> => "UINT64" });
table!(CatalogRow, "KgCatalogRevision", { revision: String => "STRING", domain: String => "STRING", language: String => "STRING", namespace: String => "STRING", member_count: u64 => "UINT64", members_digest: String => "STRING", config_fingerprint: String => "STRING", selection_fingerprint: String => "STRING" });
table!(MetadataRow, "KgFactMetadata", { id: String => "STRING", schema_version: u64 => "UINT64", schema_fingerprint: String => "STRING", facts_format_version: u64 => "UINT64", config_fingerprint: String => "STRING", selection_fingerprint: String => "STRING", content_digest: String => "STRING", rows_digest: String => "STRING" });

pub const FACT_TABLES: &[&str] = &[
    "KgFile",
    "KgEntity",
    "KgScope",
    "KgBinding",
    "KgUnresolvedRef",
    "KgAnchor",
    "KgDiagnostic",
    "KgCatalogRevision",
];
pub const STRUCTURE_TABLES: &[&str] = &["KgScope", "KgBinding", "KgDiagnostic"];
pub const STRUCTURE_RELATIONS: &[&str] = &[
    "KgScopeParent",
    "KgScopeOf",
    "KgInScope",
    "KgBindNode",
    "KgDiagOf",
];
pub const BUSINESS_RELATIONS: &[(&str, &str)] = &[
    ("KgCalls", "calls"),
    ("KgImports", "imports"),
    ("KgLinks", "links"),
];
pub const RELATIONS: &[(&str, &str, &str, &str)] = &[
    ("KgContains", "KgEntity", "KgEntity", ""),
    (
        "KgCalls",
        "KgEntity",
        "KgEntity",
        ", ref_id STRING, data STRING",
    ),
    (
        "KgImports",
        "KgEntity",
        "KgEntity",
        ", ref_id STRING, data STRING",
    ),
    (
        "KgLinks",
        "KgEntity",
        "KgEntity",
        ", ref_id STRING, data STRING",
    ),
    ("KgScopeParent", "KgScope", "KgScope", ""),
    ("KgScopeOf", "KgScope", "KgEntity", ""),
    ("KgInScope", "KgBinding", "KgScope", ""),
    ("KgBindNode", "KgBinding", "KgEntity", ""),
    ("KgDiagOf", "KgDiagnostic", "KgFile", ""),
    ("KgAnchorOf", "KgAnchor", "KgEntity", ""),
    ("KgMemberOf", "KgFile", "KgCatalogRevision", ""),
];

#[must_use]
pub fn statements() -> Vec<String> {
    let mut ddl = [
        FileRow::ddl(),
        NodeRow::ddl(),
        ScopeRow::ddl(),
        BindingRow::ddl(),
        ReferenceRow::ddl(),
        AnchorRow::ddl(),
        DiagnosticRow::ddl(),
        CatalogRow::ddl(),
        MetadataRow::ddl(),
    ]
    .into_iter()
    .collect::<Vec<_>>();
    ddl.extend(RELATIONS.iter().map(|(name, source, target, props)| {
        format!("CREATE REL TABLE {name}(FROM {source} TO {target}{props})")
    }));
    ddl
}
pub fn fingerprint() -> crate::Result<String> {
    crate::processing::stable_digest(&(SCHEMA_VERSION, statements()))
}

// Publication metadata is created only on new candidates; opening never adds DDL.
table!(RootRow, "GraphRoot", {
    graph_build_id: String => "STRING", operation_id: String => "STRING", attempt: u64 => "UINT64",
    base_text_revision: u64 => "UINT64", base_catalog_digest: String => "STRING", base_head: Option<String> => "STRING",
    scope: String => "STRING", contract_version: String => "STRING", schema_fingerprint: String => "STRING",
    rules_fingerprint: String => "STRING", module_roots_fingerprint: String => "STRING",
    configuration: String => "STRING", policy_fingerprint: String => "STRING", catalog_revisions: String => "STRING",
    facts_digest: String => "STRING", rows_digest: String => "STRING", file_count: u64 => "UINT64", node_count: u64 => "UINT64",
    reference_count: u64 => "UINT64", committed_epoch_ms: u64 => "UINT64"
});
table!(ReceiptRow, "Receipt", {
    receipt_id: String => "STRING", graph_build_id: String => "STRING", operation_id: String => "STRING",
    attempt: u64 => "UINT64", root_digest: String => "STRING", outcome: String => "STRING", stage: String => "STRING", detail_code: String => "STRING", epoch_ms: u64 => "UINT64"
});

pub(super) fn build_statements() -> Vec<String> {
    vec![RootRow::ddl(), ReceiptRow::ddl()]
}

// New batched candidates opt in at creation. No DDL is run by recovery/open.
table!(ImportRow, "KgImportProgress", {
    id: String => "STRING", format_version: u64 => "UINT64",
    plan_digest: String => "STRING", facts_digest: String => "STRING",
    max_batch_rows: u64 => "UINT64", max_batch_bytes: u64 => "UINT64",
    total_batches: u64 => "UINT64", total_rows: u64 => "UINT64",
    completed_batches: u64 => "UINT64", committed_rows: u64 => "UINT64",
    prefix_digest: String => "STRING", finalized: bool => "BOOL"
});

pub(super) fn import_statement() -> String {
    ImportRow::ddl()
}
pub fn build_fingerprint() -> crate::Result<String> {
    crate::processing::stable_digest(&(1_u32, fingerprint()?, build_statements()))
}
