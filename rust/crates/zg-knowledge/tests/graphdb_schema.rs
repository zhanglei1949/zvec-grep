//! G2.0 capability probes for the pinned binding, not the production KG schema.
//! Negative probes identify the tested forms; JSON remains an available fallback.
#![cfg(feature = "graphdb")]

use std::{sync::Arc, time::Instant};

use graphdb::{
    Batch, Column, Connection, Database, ExecutionOutcome, ImportOptions, LogicalType,
    QueryOptions, Result, Schema, TransactionMode, Value, params,
};

fn connection() -> Result<Connection> {
    Database::builder()
        .transaction_mode(TransactionMode::Tp)
        .open_in_memory()?
        .connect()
}

#[test]
fn parameterized_source_interval_lookup() -> Result<()> {
    let mut connection = connection()?;
    connection.execute("CREATE NODE TABLE IntervalProbe(id STRING PRIMARY KEY, domain STRING, file_path STRING, span_start UINT64, span_end UINT64)", ())?;
    connection.execute("CREATE (:IntervalProbe {id:'n', domain:'code', file_path:'a.rs', span_start:1, span_end:8})", ())?;
    let mut transaction = connection.transaction_read_only()?;
    let id = transaction.query_one(
        "MATCH (n:IntervalProbe) WHERE n.domain='code' AND n.file_path=$source AND n.span_start < $upper_bound AND n.span_end > $lower_bound RETURN n.id ORDER BY n.id SKIP 0 LIMIT 256",
        params! {"source"=>"a.rs", "lower_bound"=>2u64, "upper_bound"=>5u64},
        |row| row.try_get::<String>(0),
    )?;
    assert_eq!(id, "n");
    transaction.commit()?;
    connection.close()
}

#[test]
fn string_keys_uint64_lists_and_typed_nulls_round_trip() -> Result<()> {
    let mut connection = connection()?;
    connection.execute(
        "CREATE NODE TABLE Typed(id STRING PRIMARY KEY, signed INT64, unsigned UINT64, \
         enabled BOOL, score DOUBLE, note STRING, tags STRING[])",
        (),
    )?;
    assert!(!connection.in_transaction());
    let tags = Value::List {
        element_type: LogicalType::String,
        values: vec![
            Value::String("中文".into()),
            Value::Null(LogicalType::String),
        ],
    };
    let note = Value::Null(LogicalType::String);
    let mut transaction = connection.transaction()?;
    transaction.execute(
        "CREATE (:Typed {id: $id, signed: $signed, unsigned: $unsigned, enabled: $enabled, \
         score: $score, note: $note, tags: $tags})",
        params! {
            "id" => "引号'\0", "signed" => i64::MIN, "unsigned" => u64::MAX,
            "enabled" => true, "score" => 1.25_f64, "note" => &note, "tags" => &tags
        },
    )?;
    transaction.commit()?;
    let mut view = connection.transaction_read_only()?;
    view.query_one(
        "MATCH (t:Typed) WHERE t.id=$id \
         RETURN t.id, t.signed, t.unsigned, t.enabled, t.score, t.note, t.tags",
        params! {"id" => "引号'\0"},
        |row| {
            assert_eq!(row.try_get::<String>(0)?, "引号'\0");
            assert_eq!(row.try_get::<i64>(1)?, i64::MIN);
            assert_eq!(row.try_get::<u64>(2)?, u64::MAX);
            assert!(row.try_get::<bool>(3)?);
            assert!((row.try_get::<f64>(4)? - 1.25).abs() < f64::EPSILON);
            assert_eq!(row.get_owned(5)?, note);
            assert_eq!(row.get_owned(6)?, tags);
            Ok(())
        },
    )?;
    view.commit()?;
    eprintln!(
        "G2_PROBE typed columns: STRING PK, INT64, UINT64(u64::MAX), BOOL, DOUBLE, NULL, STRING[] passed"
    );
    connection.close()
}

#[test]
fn relation_properties_parallel_edges_and_reverse_reads_work() -> Result<()> {
    let mut connection = connection()?;
    connection.execute("CREATE NODE TABLE Item(id STRING PRIMARY KEY)", ())?;
    connection.execute(
        "CREATE REL TABLE Candidate(FROM Item TO Item, expected_version STRING)",
        (),
    )?;
    assert!(!connection.in_transaction());
    let mut transaction = connection.transaction()?;
    for id in ["a", "b"] {
        transaction.execute("CREATE (:Item {id: $id})", params! {"id" => id})?;
    }
    for version in ["r1", "r2"] {
        transaction.execute(
            "MATCH (a:Item), (b:Item) WHERE a.id=$a AND b.id=$b \
             CREATE (a)-[:Candidate {expected_version: $version}]->(b)",
            params! {"a" => "a", "b" => "b", "version" => version},
        )?;
    }
    transaction.commit()?;
    let mut view = connection.transaction_read_only()?;
    let versions: Vec<String> = {
        let result = view.query(
            "MATCH (b:Item)<-[r:Candidate]-(a:Item) WHERE b.id=$target \
             RETURN r.expected_version ORDER BY r.expected_version",
            params! {"target" => "b"},
        )?;
        let mut rows = result.rows();
        let mut versions = Vec::new();
        while let Some(row) = rows.next()? {
            versions.push(row.try_get(0)?);
        }
        versions
    };
    assert_eq!(versions, ["r1", "r2"]);
    view.commit()?;
    let failure = connection.execute(
        "CREATE REL TABLE InvalidEndpoint(FROM Candidate TO Item)",
        (),
    );
    assert!(
        failure.is_err(),
        "a relation table cannot be a vertex endpoint"
    );
    assert!(!connection.in_transaction());
    eprintln!(
        "G2_PROBE relations: version property, parallel edges, reverse traversal passed; REL endpoint rejected"
    );
    connection.close()
}

#[test]
fn unsupported_ddl_forms_do_not_leave_a_transaction_or_partial_schema() -> Result<()> {
    let mut connection = connection()?;
    connection.execute(
        "CREATE NODE TABLE Item(id STRING PRIMARY KEY, name STRING)",
        (),
    )?;
    connection.execute("CREATE (:Item {id: 'original', name: 'before'})", ())?;
    for (name, query) in [
        (
            "composite_pk",
            "CREATE NODE TABLE Composite(a STRING, b STRING, PRIMARY KEY(a,b))",
        ),
        ("secondary_index", "CREATE INDEX item_name ON Item(name)"),
        ("alter", "ALTER TABLE Item ADD extra STRING"),
        ("drop", "DROP TABLE Item"),
        ("merge", "MERGE (:Item {id: 'original'})"),
        (
            "create_or_replace",
            "CREATE OR REPLACE NODE TABLE Item(id STRING PRIMARY KEY)",
        ),
    ] {
        let failure = connection
            .execute(query, ())
            .expect_err("pinned DDL form is unsupported");
        assert_ne!(
            failure.execution_outcome(),
            Some(ExecutionOutcome::Committed)
        );
        assert!(!connection.in_transaction());
        assert_eq!(
            connection
                .query_one::<i64>("MATCH (i:Item) RETURN count(i)", (), |row| row.try_get(0))?,
            1
        );
        eprintln!(
            "G2_PROBE rejected {name}: kind={:?}, outcome={:?}",
            failure.kind(),
            failure.execution_outcome()
        );
    }
    let mut transaction = connection.transaction()?;
    transaction.execute("MATCH (i:Item) WHERE i.id='original' DELETE i", ())?;
    transaction.execute("CREATE (:Item {id: 'original', name: 'replacement'})", ())?;
    transaction.rollback()?;
    assert_eq!(
        connection.query_one::<String>("MATCH (i:Item) RETURN i.name", (), |row| row.try_get(0))?,
        "before"
    );
    connection.close()
}

#[test]
fn struct_column_support_is_checked_through_actual_writes() -> Result<()> {
    let mut connection = connection()?;
    match connection.execute(
        "CREATE NODE TABLE Nested(id INT64 PRIMARY KEY, detail STRUCT(name STRING, count INT64))",
        (),
    ) {
        Ok(_) => {
            let value = Value::Struct {
                fields: vec![
                    ("name".into(), LogicalType::String),
                    ("count".into(), LogicalType::Int64),
                ],
                values: vec![Value::String("中文".into()), Value::Int64(7)],
            };
            connection.execute(
                "CREATE (:Nested {id: 1, detail: $value})",
                params! {"value" => &value},
            )?;
            let actual = connection.query_one("MATCH (n:Nested) RETURN n.detail", (), |row| {
                row.get_owned(0)
            })?;
            assert_eq!(actual, value);
            eprintln!("G2_PROBE STRUCT column: DDL + parameterized write + read passed");
        }
        Err(failure) => {
            assert_ne!(
                failure.execution_outcome(),
                Some(ExecutionOutcome::Committed)
            );
            assert!(!connection.in_transaction());
            assert_eq!(connection.query("RETURN 1", ())?.row_count(), 1);
            eprintln!(
                "G2_PROBE STRUCT column rejected: kind={:?}; canonical JSON required",
                failure.kind()
            );
        }
    }
    connection.close()
}

fn import_batch(ids: Vec<i64>) -> Result<Batch> {
    let schema = Arc::new(Schema::try_new(vec![
        ("id".into(), LogicalType::Int64),
        ("name".into(), LogicalType::String),
    ])?);
    let names = ids.iter().map(|id| Some(format!("item-{id}"))).collect();
    Batch::try_new(schema, ids.len(), vec![ids.into(), Column::strings(names)])
}

#[test]
fn parameter_writes_and_bulk_import_are_measured_and_failure_is_atomic() -> Result<()> {
    let mut connection = connection()?;
    connection.execute(
        "CREATE NODE TABLE ParamItem(id INT64 PRIMARY KEY, name STRING)",
        (),
    )?;
    connection.execute(
        "CREATE NODE TABLE BulkItem(id INT64 PRIMARY KEY, name STRING)",
        (),
    )?;
    let start = Instant::now();
    let mut transaction = connection.transaction()?;
    for id in 0..128_i64 {
        transaction.execute(
            "CREATE (:ParamItem {id: $id, name: $name})",
            params! {"id" => id, "name" => format!("item-{id}")},
        )?;
    }
    transaction.commit()?;
    let parameter_time = start.elapsed();
    let start = Instant::now();
    let summary = connection.import_batches(
        "BulkItem",
        import_batch((0..128).collect())?.into_source(),
        ImportOptions::default(),
    )?;
    let bulk_time = start.elapsed();
    assert_eq!(summary.imported_rows, 128);
    assert_eq!(summary.execution_outcome, Some(ExecutionOutcome::Committed));
    assert_eq!(
        connection.query_one::<i64>("MATCH (i:ParamItem) RETURN count(i)", (), |row| row
            .try_get(0))?,
        128
    );
    assert_eq!(
        connection.query_one::<i64>("MATCH (i:BulkItem) RETURN count(i)", (), |row| row
            .try_get(0))?,
        128
    );
    let failure = connection
        .import_batches(
            "BulkItem",
            import_batch(vec![10_000, 0])?.into_source(),
            ImportOptions::default(),
        )
        .expect_err("duplicate key rolls back the import");
    assert_eq!(
        failure.execution_outcome(),
        Some(ExecutionOutcome::RolledBack)
    );
    assert_eq!(
        connection.query_one::<i64>("MATCH (i:BulkItem) RETURN count(i)", (), |row| row
            .try_get(0))?,
        128
    );
    assert_eq!(
        connection.query_one::<i64>(
            "MATCH (i:BulkItem) WHERE i.id=10000 RETURN count(i)",
            (),
            |row| row.try_get(0)
        )?,
        0
    );
    let result = connection.query_with(
        "MATCH (i:BulkItem) WHERE i.name=$name RETURN i.id",
        params! {"name" => "item-64"},
        &QueryOptions {
            profile: true,
            max_intermediate_rows: 1000,
            ..Default::default()
        },
    )?;
    assert_eq!(result.row_count(), 1);
    assert_eq!(
        result
            .rows()
            .next()?
            .expect("one filtered result")
            .try_get::<i64>(0)?,
        64
    );
    eprintln!(
        "G2_PROBE writes=128 parameter_us={} bulk_us={} (single exploratory sample, no performance gate)",
        parameter_time.as_micros(),
        bulk_time.as_micros()
    );
    let metrics = result.metrics()?;
    assert!(metrics.profile_requested());
    assert_eq!(metrics.counter("scan_predicate_candidates"), Some(128));
    eprintln!("G2_PROBE property_filter: rows=128 scan_predicate_candidates=128 result_rows=1");
    connection.close()
}
