//! Deterministic semantic fixture for a temporary graph-build benchmark.
use std::{
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};
use zg_engine::{
    ZvecGrep,
    api::index::graph::{GraphRecoveryAction, GraphRecoveryOptions},
};
use zg_knowledge::{
    graph::{GraphDbStore, GraphLimits, ImportBudget, build::BuildSpec},
    processing::{ProcessingContext, stable_digest},
    query::{ByteSpan, EvidenceRef},
    relations::bound::FactCatalog,
    semantic::{
        Episode, EpisodeState, Fact, FactObject, FactState, Identity, Mention, ObjectId,
        ScalarValue, SemanticCatalog, SemanticEntity, SemanticSource, SemanticSourceState,
        SourceAssociation, SourceEvidence, SourceRef, SourceRole,
    },
};

fn identity(kind: &str, i: usize) -> Identity {
    Identity {
        space_id: "benchmark".into(),
        id: format!("{kind}-{i}"),
    }
}

fn fixture(catalog: &FactCatalog) -> Result<SemanticCatalog, Box<dyn std::error::Error>> {
    let file = catalog
        .files
        .iter()
        .find_map(|file| file.facts.as_ref())
        .ok_or("source facts required")?;
    let owner = file.nodes.first().ok_or("source node required")?;
    let source = SourceRef {
        source_id: file.version.path.clone(),
        source_version: "benchmark-v1".into(),
    };
    let mut data = SemanticCatalog {
        sources: vec![SemanticSource {
            source: source.clone(),
            state: SemanticSourceState::Complete,
            semantic_version: Some("benchmark-v1".into()),
        }],
        ..Default::default()
    };
    for i in 0..1000 {
        let entity = identity("entity", i);
        data.entities.push(SemanticEntity {
            identity: entity.clone(),
            kind: "project".into(),
            name: format!("Project {i}"),
            aliases: vec![],
            attributes: serde_json::Map::new(),
        });
        data.episodes.push(Episode {
            identity: identity("episode", i),
            kind: "meeting".into(),
            source: source.clone(),
            context: "benchmark".into(),
            occurred_at_ms: Some(100),
            ingested_at_ms: 200,
            content_ref: format!("host:episode-{i}"),
            state: EpisodeState::Active,
        });
        let reference = format!("benchmark-mention-{i}");
        data.mentions.push(Mention {
            ref_id: reference.clone(),
            owner_id: owner.id.clone(),
            target: entity.clone(),
            evidence: EvidenceRef {
                source_id: file.version.path.clone(),
                raw_digest: file.version.raw_digest.clone(),
                decoded_digest: file.version.decoded_digest.clone(),
                span: ByteSpan {
                    start: u64::try_from(owner.span.start)?,
                    end: u64::try_from(owner.span.end)?,
                },
                reference_id: Some(reference),
            },
        });
        for (kind, object) in [
            (
                "scalar",
                FactObject::Scalar {
                    value: ScalarValue::String(format!("Description {i}")),
                },
            ),
            (
                "relation",
                FactObject::Entity {
                    identity: identity("entity", (i + 1) % 1000),
                },
            ),
        ] {
            let fact = identity(kind, i);
            data.facts.push(Fact {
                identity: fact.clone(),
                subject: entity.clone(),
                predicate: kind.into(),
                object,
                context: "benchmark".into(),
                state: FactState::Active,
                supersedes: None,
                changed_at_ms: 200,
                attributes: serde_json::Map::new(),
            });
            data.associations.push(SourceAssociation {
                target: ObjectId::Fact { identity: fact },
                evidence: SourceEvidence {
                    source: source.clone(),
                    locator: None,
                },
                role: SourceRole::Support,
            });
        }
        data.associations.push(SourceAssociation {
            target: ObjectId::SemanticEntity { identity: entity },
            evidence: SourceEvidence {
                source: source.clone(),
                locator: None,
            },
            role: SourceRole::Support,
        });
    }
    data.normalize()?;
    Ok(data)
}

fn temporary_root() -> Result<PathBuf, Box<dyn std::error::Error>> {
    let mut args = std::env::args_os().skip(1);
    let root = PathBuf::from(args.next().ok_or("temporary workspace required")?).canonicalize()?;
    let mut temporary_roots = vec![std::env::temp_dir().canonicalize()?];
    if let Ok(public_temporary) = PathBuf::from("/tmp").canonicalize() {
        temporary_roots.push(public_temporary);
    }
    if args.next().is_some()
        || !temporary_roots
            .iter()
            .any(|base| root != *base && root.starts_with(base))
    {
        return Err("usage: graph_semantic_seed TEMPORARY_WORKSPACE".into());
    }
    Ok(root)
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let root = temporary_root()?;
    let manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(root.join(".zvec-grep/manifest.json"))?)?;
    let generation = manifest["storageGeneration"]
        .as_str()
        .ok_or("storage generation required")?;
    uuid::Uuid::parse_str(generation)?;
    let home = root.join(".zvec-grep/generations").join(generation);
    let head: serde_json::Value =
        serde_json::from_slice(&std::fs::read(home.join("graph/HEAD.json"))?)?;
    let old = head["graphBuildId"]
        .as_str()
        .ok_or("selected graph required")?;
    uuid::Uuid::parse_str(old)?;
    let builds = home.join("graph/builds");
    let mut record: serde_json::Value =
        serde_json::from_slice(&std::fs::read(builds.join(old).join("build.json"))?)?;
    let context = ProcessingContext {
        max_input_bytes: 256 << 20,
        max_output_items: 30_000_000,
        deadline: None,
        cancellation: tokio_util::sync::CancellationToken::new(),
    };
    let limits = GraphLimits {
        max_rows: 4_000_000,
        ..Default::default()
    };
    let store = GraphDbStore::open(builds.join(old).join("db"), true, limits.clone())?;
    let catalog = store.read_facts(&context)?;
    let semantic = fixture(&catalog)?;
    let semantic_digest = stable_digest(&semantic)?;
    let catalog = catalog.with_semantic(semantic, &context)?;
    drop(store);
    let id = uuid::Uuid::new_v4().to_string();
    record["graphBuildId"] = id.clone().into();
    record["operationId"] = uuid::Uuid::new_v4().to_string().into();
    record["attempt"] = 1.into();
    record["baseHead"] = old.into();
    record["createdEpochMs"] =
        u64::try_from(SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis())?.into();
    let fields = [
        ("graph_build_id", "graphBuildId"),
        ("operation_id", "operationId"),
        ("attempt", "attempt"),
        ("base_text_revision", "baseTextRevision"),
        ("base_catalog_digest", "baseCatalogDigest"),
        ("base_head", "baseHead"),
        ("scope", "scope"),
        ("configuration", "configuration"),
        ("policy_fingerprint", "policyFingerprint"),
        ("rules_fingerprint", "rulesFingerprint"),
        ("module_roots_fingerprint", "moduleRootsFingerprint"),
    ];
    let spec: BuildSpec = serde_json::from_value(serde_json::Value::Object(
        fields
            .into_iter()
            .map(|(key, input)| (key.into(), record[input].clone()))
            .collect(),
    ))?;
    let budget: ImportBudget = serde_json::from_value(record["importBudget"].clone())?;
    let path = builds.join(&id);
    std::fs::create_dir(&path)?;
    std::fs::write(path.join("build.json"), serde_json::to_vec_pretty(&record)?)?;
    let store = GraphDbStore::create_batched_build(path.join("db"), limits)?;
    store.write_facts_batched(&catalog, &budget, &context)?;
    store.commit_build(&spec, &context)?;
    store.checkpoint()?;
    drop(store);
    let engine = ZvecGrep::new();
    let options = GraphRecoveryOptions {
        root: Some(root),
        graph_build_id: id,
        generation: Some(generation.into()),
        ..Default::default()
    };
    let inspected = engine.recover_graph(options.clone()).await?;
    let selected = engine
        .recover_graph(GraphRecoveryOptions {
            action: GraphRecoveryAction::Select,
            expected_operation_id: Some(inspected.operation_id),
            expected_attempt: Some(inspected.attempt),
            ..options
        })
        .await?;
    if !selected.selected || !selected.eligible {
        return Err("semantic fixture not selected".into());
    }
    println!(
        "{}",
        serde_json::json!({"semantic_digest":semantic_digest,"selected":true,"entities":1000,"episodes":1000,"facts":2000,"associations":3000,"mentions":1000})
    );
    Ok(())
}
