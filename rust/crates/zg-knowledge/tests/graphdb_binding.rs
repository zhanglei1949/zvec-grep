//! Binding smoke tests for the upstream graphdb Rust crate.
//!
//! These cover the first admission gate in
//! `knowledge/GRAPHDB_RUST_BINDING_REQUIREMENTS.md`: parameter round-trip, DDL
//! on an idle connection, write-transaction rollback, an explicit read-only
//! view, and close-then-reopen. They deliberately stop short of the
//! production graph adapter, joint publication, incremental invalidation and
//! recovery reconciliation that G2 requires.
//!
//! Off unless the `graphdb` feature is enabled, because building the bundled
//! native library needs `CMake` 3.28+, a C++17 toolchain and the initialized
//! kuzu/nanoarrow/parallel-hashmap submodules.
#![cfg(feature = "graphdb")]

use std::{path::Path, time::Duration};

use graphdb::{
    AccessMode, BuildInfo, CancelToken, Connection, Database, ErrorKind, ErrorStage,
    ExecutionOutcome, MutationErrorCode, QueryOptions, Resource, Result, StorageMode,
    TransactionErrorCode, TransactionMode, params,
};

const DDL: &str = "CREATE NODE TABLE Person(id INT64 PRIMARY KEY, name STRING)";

/// Opens a persistent TP database. A published knowledge graph needs TP mode
/// so an explicit read-only view can coexist with a writer.
fn open_persistent(path: &Path) -> Result<Database> {
    Database::builder()
        .storage_mode(StorageMode::File)
        .access_mode(AccessMode::ReadWrite)
        .transaction_mode(TransactionMode::Tp)
        .open(path)
}

fn create_schema(connection: &mut Connection) -> Result<()> {
    let summary = connection.execute(DDL, ())?;
    assert_eq!(summary.execution_outcome, Some(ExecutionOutcome::Committed));
    Ok(())
}

/// Binds values through `params!` instead of interpolating them into the
/// statement text, which the admission requirements call out explicitly.
fn insert_person(connection: &mut Connection, id: i64, name: &str) -> Result<()> {
    let mut transaction = connection.transaction()?;
    transaction.execute(
        "CREATE (:Person {id: $id, name: $name})",
        params! { "id" => id, "name" => name },
    )?;
    transaction.commit()
}

fn person_count(connection: &mut Connection) -> Result<i64> {
    let result = connection.query("MATCH (p:Person) RETURN count(p) AS total", ())?;
    let mut rows = result.rows();
    let row = rows.next()?.expect("an aggregate always returns one row");
    let total = row.try_get::<i64>("total")?;
    assert!(
        rows.next()?.is_none(),
        "count(p) must yield exactly one row"
    );
    Ok(total)
}

/// Exercises both native libraries, not merely their link metadata. This is a
/// temporary admission fixture, not a production KG collection or joint commit.
#[test]
fn native_binding_links_beside_the_host_zvec_library() -> IntegrationResult {
    let build = BuildInfo::current()?;
    assert!(!build.identity.is_empty());
    assert_eq!(build.abi_version >> 16, 1);
    assert!(build.abi_version & 0xffff >= 3);
    assert!(build.capabilities.memory_tp());
    assert!(build.capabilities.file_storage());
    assert!(build.capabilities.tp_persistence());
    eprintln!(
        "native coexistence: graphdb ABI={:#x}, identity={}, zvec={}",
        build.abi_version,
        String::from_utf8_lossy(&build.identity),
        zvec_rust::version()
    );
    zvec_rust::initialize(Some(
        &zvec_rust::ConfigBuilder::new()
            .num_threads(2)
            .memory_limit(512 * 1024 * 1024),
    ))?;
    let directory = tempfile::tempdir()?;
    let graph_path = directory.path().join("graph");
    let text_path = directory.path().join("text");
    let database = open_persistent(&graph_path)?;
    let text = create_text_collection(&text_path)?;
    write_coexisting_stores(&database, &text)?;
    assert_graph_text_round_trip(&database, &text)?;
    text.flush()?;
    let mut connection = database.connect()?;
    connection.checkpoint()?;
    connection.close()?;
    drop(database);
    text.close()?;

    // Reverse open order, with both handles alive again. No Python, model,
    // alternate DB, projection mock, or global zvec shutdown is involved.
    let text =
        zg_storage::zvec::open_collection(text_path.to_str().expect("UTF-8 temporary path"), true)?;
    let database = Database::builder()
        .storage_mode(StorageMode::File)
        .access_mode(AccessMode::ReadOnly)
        .transaction_mode(TransactionMode::Tp)
        .open(&graph_path)?;
    assert_graph_text_round_trip(&database, &text)?;
    text.close()?;
    drop(database);
    Ok(())
}

#[test]
fn ddl_runs_on_an_idle_connection() -> Result<()> {
    let database = Database::open_in_memory()?;
    let mut connection = database.connect()?;
    create_schema(&mut connection)?;
    assert!(
        !connection.in_transaction(),
        "DDL on an idle connection must not leave a transaction open"
    );
    connection.close()?;
    drop(database);
    Ok(())
}

#[test]
fn parameters_round_trip_through_a_committed_write() -> Result<()> {
    let database = Database::open_in_memory()?;
    let mut connection = database.connect()?;
    create_schema(&mut connection)?;
    insert_person(&mut connection, 42, "Alice")?;

    let result = connection.query("MATCH (p:Person) RETURN p.id AS id, p.name AS name", ())?;
    connection.close()?;

    let mut rows = result.rows();
    let row = rows.next()?.expect("the committed person must be visible");
    assert_eq!(row.try_get::<i64>("id")?, 42, "INT64 must round-trip");
    assert_eq!(
        row.get_ref("name")?.as_str()?,
        "Alice",
        "STRING must round-trip as UTF-8"
    );
    assert!(rows.next()?.is_none(), "exactly one person was committed");

    drop(database);
    Ok(())
}

#[test]
fn read_only_transaction_sees_committed_writes() -> Result<()> {
    let database = Database::open_in_memory()?;
    let mut connection = database.connect()?;
    create_schema(&mut connection)?;
    insert_person(&mut connection, 1, "Ada")?;

    // An explicit READ ONLY transaction, never a default Update transaction.
    let mut view = connection.transaction_read_only()?;
    let result = view.query("MATCH (p:Person) RETURN p.name AS name", ())?;
    view.commit()?;

    let mut rows = result.rows();
    let row = rows
        .next()?
        .expect("the read-only view must see the commit");
    assert_eq!(row.get_ref("name")?.as_str()?, "Ada");
    assert!(rows.next()?.is_none());

    connection.close()?;
    drop(database);
    Ok(())
}

#[test]
fn rolled_back_write_leaves_no_business_effect() -> Result<()> {
    let database = Database::open_in_memory()?;
    let mut connection = database.connect()?;
    create_schema(&mut connection)?;

    let mut transaction = connection.transaction()?;
    transaction.execute(
        "CREATE (:Person {id: $id, name: $name})",
        params! { "id" => 7_i64, "name" => "Ghost" },
    )?;
    transaction.rollback()?;

    assert_eq!(
        person_count(&mut connection)?,
        0,
        "a rolled-back transaction must leave no business effect"
    );

    connection.close()?;
    drop(database);
    Ok(())
}

#[test]
fn close_and_reopen_preserves_committed_data() -> Result<()> {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let path = directory.path().join("knowledge-graph");

    // The native directory lock rejects a second open of the same path, even
    // in this process, so the first instance must be fully closed first.
    {
        let database = open_persistent(&path)?;
        let mut connection = database.connect()?;
        create_schema(&mut connection)?;
        insert_person(&mut connection, 7, "Grace")?;
        connection.close()?;
        drop(database);
    }

    let database = open_persistent(&path)?;
    let mut connection = database.connect()?;
    let result = connection.query("MATCH (p:Person) RETURN p.name AS name", ())?;
    connection.close()?;

    let mut rows = result.rows();
    let row = rows
        .next()?
        .expect("the committed person must survive a reopen");
    assert_eq!(row.get_ref("name")?.as_str()?, "Grace");
    assert!(rows.next()?.is_none());

    drop(database);
    Ok(())
}

type IntegrationResult<T = ()> = std::result::Result<T, Box<dyn std::error::Error>>;

fn create_text_collection(path: &Path) -> IntegrationResult<zvec_rust::Collection> {
    use zvec_rust::{CollectionSchema, DataType, FieldSchema, IndexParams, MetricType};

    let mut schema = CollectionSchema::new("g2_admission")?;
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
    Ok(zg_storage::zvec::create_collection(
        path.to_str().expect("UTF-8 temporary path"),
        &schema,
    )?)
}

fn write_coexisting_stores(database: &Database, text: &zvec_rust::Collection) -> IntegrationResult {
    let mut docs = Vec::new();
    for (id, body, vector) in [
        ("doc-a", "knowledge first 中文", [1.0, 0.0, 0.0, 0.0]),
        ("doc-b", "knowledge second 说明", [0.0, 1.0, 0.0, 0.0]),
    ] {
        let mut doc = zvec_rust::Doc::new()?;
        doc.try_set_pk(id)?;
        doc.add_string("revision", "r1")?;
        doc.add_string("body", body)?;
        doc.add_vector_f32("embedding", &vector)?;
        docs.push(doc);
    }
    let written = text.insert(&docs.iter().collect::<Vec<_>>())?;
    assert!(zg_storage::zvec::write_counts_match(
        2,
        written.results.iter().map(|item| item.success),
        written.success_count,
        written.error_count,
    ));
    assert_eq!(written.success_count, 2);
    assert_eq!(written.error_count, 0);
    assert!(written.results.iter().all(|item| item.success));

    let mut connection = database.connect()?;
    connection.execute(
        "CREATE NODE TABLE Document(doc_id STRING PRIMARY KEY, revision STRING)",
        (),
    )?;
    connection.execute(
        "CREATE REL TABLE RefersTo(FROM Document TO Document, source_revision STRING)",
        (),
    )?;
    let mut transaction = connection.transaction()?;
    for id in ["doc-a", "doc-b"] {
        transaction.execute(
            "CREATE (:Document {doc_id: $id, revision: $revision})",
            params! {"id" => id, "revision" => "r1"},
        )?;
    }
    transaction.execute(
        "MATCH (a:Document), (b:Document) WHERE a.doc_id=$source AND b.doc_id=$target \
         CREATE (a)-[:RefersTo {source_revision: $revision}]->(b)",
        params! {"source" => "doc-a", "target" => "doc-b", "revision" => "r1"},
    )?;
    transaction.commit()?;
    connection.close()?;
    Ok(())
}

fn assert_graph_text_round_trip(
    database: &Database,
    text: &zvec_rust::Collection,
) -> IntegrationResult {
    let mut connection = database.connect()?;
    let mut view = connection.transaction_read_only()?;
    let (target, revision, source_revision) = view.query_one(
        "MATCH (a:Document)-[r:RefersTo]->(b:Document) WHERE a.doc_id=$source \
         RETURN b.doc_id AS target, b.revision AS revision, r.source_revision AS source_revision",
        params! {"source" => "doc-a"},
        |row| {
            Ok((
                row.try_get::<String>("target")?,
                row.try_get::<String>("revision")?,
                row.try_get::<String>("source_revision")?,
            ))
        },
    )?;
    view.commit()?;
    connection.close()?;
    assert_eq!(target, "doc-b");
    assert_eq!(revision, "r1");
    assert_eq!(source_revision, "r1");
    let docs = text.fetch_with_options(&[&target], None, true)?;
    assert_eq!(docs.len(), 1);
    assert_eq!(docs[0].get_pk(), Some(target.as_str()));
    assert_eq!(docs[0].get_string("revision")?.as_deref(), Some("r1"));
    assert_eq!(
        docs[0].get_string("body")?.as_deref(),
        Some("knowledge second 说明")
    );
    assert_eq!(
        docs[0].get_vector_f32("embedding")?.as_deref(),
        Some([0.0, 1.0, 0.0, 0.0].as_slice())
    );
    let mut query = zvec_rust::SearchQuery::new("embedding", &[0.0, 1.0, 0.0, 0.0], 1)?;
    query.set_filter("revision = 'r1'")?;
    let hits = text.query(&query)?;
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].get_pk(), Some(target.as_str()));
    assert!(hits[0].get_score().is_finite());
    let mut fts = zvec_rust::Fts::new()?;
    fts.set_query_string("knowledge")?;
    let mut query = zvec_rust::SearchQuery::fts("body", &fts, 10)?;
    query.set_filter("revision = 'r1'")?;
    let hits = text.query(&query)?;
    let mut ids = hits.iter().map(|doc| doc.get_pk()).collect::<Vec<_>>();
    ids.sort_unstable();
    assert_eq!(ids, vec![Some("doc-a"), Some("doc-b")]);
    query.set_filter("revision = 'not-selected'")?;
    assert!(text.query(&query)?.is_empty());
    Ok(())
}

#[test]
fn old_read_view_is_fixed_and_new_read_view_confirms_a_commit() -> IntegrationResult {
    let directory = tempfile::tempdir()?;
    let database = open_persistent(&directory.path().join("graph"))?;
    let mut writer = database.connect()?;
    create_schema(&mut writer)?;
    let mut old_connection = database.connect()?;
    let mut old = old_connection.transaction_read_only()?;
    assert_eq!(
        old.query_one::<i64>("MATCH (p:Person) RETURN count(p)", (), |row| row.try_get(0))?,
        0
    );
    insert_person(&mut writer, 1, "new publication")?;
    assert_eq!(
        old.query_one::<i64>("MATCH (p:Person) RETURN count(p)", (), |row| row.try_get(0))?,
        0,
        "an already established read view must remain fixed"
    );
    let mut fresh_connection = database.connect()?;
    let mut fresh = fresh_connection.transaction_read_only()?;
    assert_eq!(
        fresh.query_one::<i64>("MATCH (p:Person) RETURN count(p)", (), |row| row.try_get(0))?,
        1,
        "a new explicit read transaction, not the commit response, confirms visibility"
    );
    fresh.commit()?;
    old.commit()?;
    fresh_connection.close()?;
    old_connection.close()?;
    writer.close()?;
    drop(database);
    Ok(())
}

#[test]
fn failed_write_does_not_commit_an_earlier_partial_effect() -> Result<()> {
    let database = Database::open_in_memory()?;
    let mut connection = database.connect()?;
    create_schema(&mut connection)?;
    let mut transaction = connection.transaction()?;
    transaction.execute("CREATE (:Person {id: 1, name: 'first'})", ())?;
    let failure = transaction
        .execute("CREATE (:Person {id: 1, name: 'duplicate'})", ())
        .expect_err("duplicate primary key must fail");
    assert_eq!(
        failure.mutation_code(),
        Some(MutationErrorCode::DuplicateKey)
    );
    assert_eq!(
        failure.execution_outcome(),
        Some(ExecutionOutcome::InTransaction {
            rollback_only: true
        })
    );
    assert!(transaction.commit().is_err());
    assert!(!connection.in_transaction());
    assert_eq!(person_count(&mut connection)?, 0);
    connection.close()?;
    Ok(())
}

#[test]
fn read_only_reopen_rejects_writes_and_keeps_committed_data() -> IntegrationResult {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("graph");
    let database = open_persistent(&path)?;
    let mut connection = database.connect()?;
    create_schema(&mut connection)?;
    insert_person(&mut connection, 1, "read-only")?;
    connection.close()?;
    drop(database);
    let read_only = Database::builder()
        .storage_mode(StorageMode::File)
        .access_mode(AccessMode::ReadOnly)
        .transaction_mode(TransactionMode::Tp)
        .open(&path)?;
    let mut connection = read_only.connect()?;
    assert_eq!(person_count(&mut connection)?, 1);
    let failure = connection
        .execute("CREATE (:Person {id: 2, name: 'forbidden'})", ())
        .expect_err("read-only storage must reject a write");
    assert!(
        failure.transaction_code() == Some(TransactionErrorCode::ReadOnly)
            || failure.mutation_code() == Some(MutationErrorCode::ReadOnly)
    );
    assert_ne!(
        failure.execution_outcome(),
        Some(ExecutionOutcome::Committed)
    );
    connection.close()?;
    drop(read_only);
    let database = open_persistent(&path)?;
    let mut connection = database.connect()?;
    assert_eq!(person_count(&mut connection)?, 1);
    connection.close()?;
    Ok(())
}

#[test]
fn persistent_directory_stays_owned_until_all_handles_are_released() -> IntegrationResult {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("graph");
    let database = open_persistent(&path)?;
    let shared = database.clone();
    let mut connection = database.connect()?;
    create_schema(&mut connection)?;
    drop(database);
    assert!(open_persistent(&path).is_err());
    drop(shared);
    assert!(
        open_persistent(&path).is_err(),
        "connection retains ownership"
    );
    assert_eq!(person_count(&mut connection)?, 0);
    connection.close()?;
    let reopened = open_persistent(&path)?;
    let mut connection = reopened.connect()?;
    assert_eq!(person_count(&mut connection)?, 0);
    connection.close()?;
    Ok(())
}

#[test]
fn cancellation_and_result_budget_keep_structured_outcomes() -> Result<()> {
    let database = Database::open_in_memory()?;
    let mut connection = database.connect()?;
    let token = CancelToken::new()?;
    token.cancel();
    let failure = connection
        .query_with(
            "RETURN 1",
            (),
            &QueryOptions {
                cancel: Some(token),
                ..Default::default()
            },
        )
        .expect_err("cancelled query must fail");
    assert_eq!(failure.kind(), ErrorKind::Cancelled);
    assert_ne!(
        failure.execution_outcome(),
        Some(ExecutionOutcome::Committed)
    );
    let failure = connection
        .query_with(
            "UNWIND [1, 2] AS n RETURN n",
            (),
            &QueryOptions {
                max_results: 1,
                ..Default::default()
            },
        )
        .expect_err("result row budget must be enforced");
    assert_eq!(failure.kind(), ErrorKind::ResourceExhausted);
    assert_eq!(failure.resource(), Some((Resource::ResultRows, 1)));
    assert_eq!(
        failure.execution_outcome(),
        Some(ExecutionOutcome::RolledBack)
    );
    assert_eq!(connection.query("RETURN 1", ())?.row_count(), 1);
    connection.close()?;
    Ok(())
}

#[test]
fn admission_timeout_is_not_an_execution_timeout() -> Result<()> {
    // AP deliberately exercises admission serialization. TP read-view coexistence
    // is tested separately; this does not claim a running-query deadline.
    let database = Database::open_in_memory()?;
    let mut first = database.connect()?;
    let mut second = database.connect()?;
    let transaction = first.transaction()?;
    let failure = second
        .query_with(
            "RETURN 1",
            (),
            &QueryOptions {
                transaction_timeout: Duration::from_millis(1),
                ..Default::default()
            },
        )
        .expect_err("a held AP transaction must exhaust admission");
    assert_eq!(failure.kind(), ErrorKind::AdmissionTimeout);
    assert_eq!(
        failure.execution_outcome(),
        Some(ExecutionOutcome::NotStarted)
    );
    transaction.rollback()?;
    assert_eq!(second.query("RETURN 1", ())?.row_count(), 1);
    first.close()?;
    second.close()?;
    Ok(())
}

#[test]
fn result_conversion_failure_does_not_turn_a_commit_into_a_retry() -> Result<()> {
    let database = Database::open_in_memory()?;
    let mut connection = database.connect()?;
    create_schema(&mut connection)?;
    let failure = connection
        .query_one::<i64>(
            "UNWIND [1, 2] AS id CREATE (:Person {id: id, name: 'committed'}) RETURN id",
            (),
            |row| row.try_get(0),
        )
        .expect_err("query_one rejects the two returned rows after the write");
    assert_eq!(failure.kind(), ErrorKind::InvalidArgument);
    assert_eq!(failure.stage(), ErrorStage::Result);
    assert_eq!(
        failure.execution_outcome(),
        Some(ExecutionOutcome::Committed)
    );
    assert_eq!(person_count(&mut connection)?, 2);
    connection.close()?;
    Ok(())
}
