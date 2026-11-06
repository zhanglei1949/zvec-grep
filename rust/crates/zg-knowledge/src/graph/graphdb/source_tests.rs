use super::semantic_tests::semantic_fixture;
use super::tests::{bind, context, version};
use super::*;
use crate::{
    relations::{FileFacts, Language},
    semantic::*,
};

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;
fn changed_source(base: &FactCatalog) -> TestResult<FactCatalog> {
    let mut files = base.files.clone();
    let old = files
        .iter_mut()
        .find(|f| f.record.path == "a.py")
        .ok_or("source")?;
    let text = "#\n";
    let module = old.facts.as_ref().ok_or("facts")?.module.clone();
    *old = bind(
        FileFacts::new(version("a.py", text), Language::Python, module, text)?,
        text,
        old.record.file_id,
    )?;
    Ok(FactCatalog::resolve(
        files,
        base.config_fingerprint.clone(),
        base.selection_fingerprint.clone(),
        &context(),
    )?)
}
fn change(catalog: &FactCatalog, update: SemanticUpdate) -> TestResult<FactCatalog> {
    Ok(catalog.clone().apply_sources(
        &SemanticInput {
            episode_links: Vec::new(),
            space_id: "workspace".into(),
            update_scope: vec!["a.py".into()],
            sources: vec![SourceChange::Upsert {
                source: SourceRef {
                    source_id: "a.py".into(),
                    source_version: "v2".into(),
                },
                semantic: Some(update),
            }],
            episodes: vec![],
            fact_changes: vec![],
            episode_withdrawals: vec![],
        },
        &context(),
    )?)
}
fn batch() -> SemanticBatch {
    SemanticBatch {
        entities: vec![],
        facts: vec![],
        mentions: vec![],
        associations: vec![],
    }
}
fn persist(path: &std::path::Path, catalog: &FactCatalog) -> TestResult<GraphDbStore> {
    let (_, mut spec) = semantic_fixture()?;
    spec.base_catalog_digest = crate::processing::stable_digest(
        &catalog.files.iter().map(|f| &f.record).collect::<Vec<_>>(),
    )?;
    let store = GraphDbStore::create_batched_build(path, GraphLimits::default())?;
    store.write_facts_batched(
        catalog,
        &ImportBudget {
            max_batch_rows: 3,
            max_batch_bytes: 4096,
        },
        &context(),
    )?;
    let counts = store.counts()?;
    let proof = store.commit_build(&spec, &context())?;
    assert_eq!(
        store.verify_rows(&context())?,
        codec::FactRows::encode(catalog)?.digest()?
    );
    assert_eq!(
        codec::catalog_digest(catalog)?,
        crate::processing::stable_digest(catalog)?
    );
    store.checkpoint()?;
    drop(store);
    let read = GraphDbStore::open(path, true, GraphLimits::default())?;
    assert_eq!(read.read_facts(&context())?, *catalog);
    assert_eq!(read.counts()?, counts);
    assert_eq!(read.inspect_build(&spec, &context())?, Some(proof));
    Ok(read)
}
#[test]
fn changed_source_retains_old_positions_only_as_auxiliaries_until_complete() -> TestResult {
    let (base, _) = semantic_fixture()?;
    let original = base.semantic.mentions[0].clone();
    let inherited = changed_source(&base)?.inherit_semantic(&base, &[], &context())?;
    assert!(inherited.semantic.mentions.is_empty());
    assert_eq!(inherited.semantic.deferred_mentions, vec![original.clone()]);
    assert_eq!(inherited.semantic.facts, base.semantic.facts);
    assert_eq!(inherited.semantic.episodes, base.semantic.episodes);
    let partial = change(&inherited, SemanticUpdate::Partial(batch()))?;
    assert_eq!(partial.semantic.deferred_mentions, vec![original]);
    let dir = tempfile::tempdir()?;
    let read = persist(&dir.path().join("partial"), &partial)?;
    assert_eq!(read.counts()?.nodes["KgDeferredMention"], 1);
    assert_eq!(read.counts()?.relations["KgMentions"], 0);
    drop(read);
    let mut fresh = batch();
    let mut mention = base.semantic.mentions[0].clone();
    let facts = inherited
        .files
        .iter()
        .find_map(|f| f.facts.as_ref().filter(|f| f.version.path == "a.py"))
        .ok_or("new facts")?;
    mention.owner_id = facts.nodes[0].id.clone();
    mention.evidence.raw_digest = facts.version.raw_digest.clone();
    mention.evidence.decoded_digest = facts.version.decoded_digest.clone();
    mention.evidence.span.end = 1;
    fresh.mentions.push(mention);
    let complete = change(&partial, SemanticUpdate::Complete(fresh))?;
    assert_eq!(complete.semantic.mentions.len(), 1);
    assert!(complete.semantic.deferred_mentions.is_empty());
    let read = persist(&dir.path().join("complete"), &complete)?;
    assert_eq!(read.counts()?.nodes["KgDeferredMention"], 0);
    assert_eq!(read.counts()?.relations["KgMentions"], 1);
    assert!(complete.semantic.current_facts().next().is_none());
    assert!(
        complete
            .semantic
            .associations
            .iter()
            .any(|a| a.role == SourceRole::Withdrawn && matches!(a.target, ObjectId::Fact { .. }))
    );
    Ok(())
}
#[test]
fn inherited_source_deletion_is_explicit_and_does_not_remove_other_origins() -> TestResult {
    let (base, _) = semantic_fixture()?;
    let mut files = base.files.clone();
    files.retain(|f| f.record.path != "a.py");
    let rebuilt = FactCatalog::resolve(
        files,
        base.config_fingerprint.clone(),
        base.selection_fingerprint.clone(),
        &context(),
    )?;
    let merely_missing = rebuilt.clone().inherit_semantic(&base, &[], &context())?;
    assert_eq!(
        merely_missing.semantic.current_facts().count(),
        base.semantic.current_facts().count()
    );
    assert_eq!(
        merely_missing.semantic.deferred_mentions,
        base.semantic.mentions
    );
    let reference = base
        .semantic
        .sources
        .iter()
        .find(|s| s.source.source_id == "a.py")
        .ok_or("source")?
        .source
        .clone();
    let repeated = SourceChange::Delete {
        source: reference.clone(),
    };
    assert!(
        rebuilt
            .clone()
            .inherit_semantic(&base, &[repeated.clone(), repeated], &context())
            .is_err()
    );
    let removed = rebuilt.inherit_semantic(
        &base,
        &[SourceChange::Delete { source: reference }],
        &context(),
    )?;
    assert_eq!(removed.semantic.entities, base.semantic.entities);
    assert_eq!(removed.semantic.episodes, base.semantic.episodes);
    assert!(removed.semantic.current_facts().next().is_none());
    assert!(removed.semantic.deferred_mentions.is_empty());
    assert!(
        removed
            .semantic
            .sources
            .iter()
            .any(|s| s.source.source_id == "a.py" && s.state == SemanticSourceState::Deleted)
    );
    assert!(removed.semantic.sources.iter().any(
        |s| s.source.source_id == "episode:meeting" && s.state == SemanticSourceState::Complete
    ));
    let dir = tempfile::tempdir()?;
    persist(&dir.path().join("removed"), &removed)?;
    Ok(())
}
#[test]
fn malformed_complete_positions_and_deferred_row_tampering_are_rejected() -> TestResult {
    let (base, _) = semantic_fixture()?;
    let inherited = changed_source(&base)?.inherit_semantic(&base, &[], &context())?;
    let mut invalid = batch();
    invalid.mentions = base.semantic.mentions.clone();
    assert!(change(&inherited, SemanticUpdate::Complete(invalid)).is_err());
    for mutation in [
        "MATCH(n:KgDeferredMention) SET n.data='{}'",
        "MATCH(n:KgDeferredMention) DELETE n",
        "CREATE(:KgDeferredMention{id:'extra',data:'{}'})",
    ] {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("graph");
        drop(persist(&path, &inherited)?);
        let store = GraphDbStore::open(&path, false, GraphLimits::default())?;
        store.with_write(|tx, opts| {
            tx.execute_with(mutation, (), opts)?;
            Ok(())
        })?;
        assert!(store.read_facts(&context()).is_err(), "{mutation}");
    }
    Ok(())
}
