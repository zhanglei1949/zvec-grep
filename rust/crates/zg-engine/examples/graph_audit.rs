//! Offline maintenance audit of an already selected candidate. Never used by queries.
use std::{collections::BTreeMap, path::PathBuf, time::Instant};
use zg_knowledge::{
    graph::{GraphDbStore, GraphLimits, build::BuildSpec},
    processing::{ProcessingContext, stable_digest},
    relations::{
        Lookup, RelationKind,
        bound::{FactCatalog, TargetVersion},
    },
};

fn target_projection(target: &TargetVersion) -> serde_json::Value {
    serde_json::json!({"node_id":target.node_id,"path":target.source.path,"raw_digest":target.source.raw_digest,"decoded_digest":target.source.decoded_digest})
}

fn source_projection(catalog: &FactCatalog) -> Result<serde_json::Value, serde_json::Error> {
    let files = catalog.files.iter().map(|bound| {
        let mut facts = serde_json::to_value(&bound.facts)?;
        if let Some(facts) = facts.as_object_mut() {
            // Rule revisions and generation fences are verified independently;
            // they must differ when comparing builds of different contracts.
            facts.remove("rules_fingerprint");
            facts["version"].as_object_mut().expect("source version").remove("generation");
        }
        Ok(serde_json::json!({"path":bound.record.path,"content_hash":bound.record.content_hash,"facts":facts}))
    }).collect::<Result<Vec<_>, serde_json::Error>>()?;
    let resolutions = catalog.resolutions.iter().map(|r| serde_json::json!({"source":r.file_path,"reference":r.reference_id,"status":r.status,"reason":r.reason,"targets":r.targets.iter().map(target_projection).collect::<Vec<_>>(),"relation_source":r.relation_source.as_ref().map(target_projection)})).collect::<Vec<_>>();
    Ok(
        serde_json::json!({"version":"source-projection-v1","files":files,"resolutions":resolutions}),
    )
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args_os().skip(1);
    let workspace = PathBuf::from(args.next().ok_or("workspace required")?);
    let database = PathBuf::from(args.next().ok_or("database required")?);
    let spec: BuildSpec =
        serde_json::from_slice(&std::fs::read(args.next().ok_or("spec required")?)?)?;
    let output = PathBuf::from(args.next().ok_or("output required")?);
    if args.next().is_some() {
        return Err("usage: WORKSPACE DATABASE SPEC_JSON OUTPUT_JSON".into());
    }
    let context = ProcessingContext {
        max_input_bytes: 256 << 20,
        max_output_items: 30_000_000,
        deadline: None,
        cancellation: tokio_util::sync::CancellationToken::new(),
    };
    let started = Instant::now();
    let store = GraphDbStore::open(
        database,
        true,
        GraphLimits {
            max_rows: 4_000_000,
            ..Default::default()
        },
    )?;
    let proof = store
        .inspect_published(&spec, &context)?
        .ok_or("published proof mismatch")?;
    let catalog = store.read_facts(&context)?;
    let projection_digest = stable_digest(&source_projection(&catalog)?)?;
    let resolutions = catalog
        .resolutions
        .iter()
        .map(|r| (r.reference_id.as_str(), r))
        .collect::<BTreeMap<_, _>>();
    let mut counts = BTreeMap::<String, u64>::new();
    let mut reasons = BTreeMap::<String, u64>::new();
    let mut shapes = BTreeMap::<String, u64>::new();
    let mut references = Vec::new();
    let mut source_states = Vec::new();
    let mut languages = Vec::new();
    for bound in &catalog.files {
        let Some(facts) = &bound.facts else {
            continue;
        };
        if !languages.contains(&facts.language) {
            languages.push(facts.language);
        }
        source_states.push(serde_json::json!({"path":bound.record.path,"complete":facts.complete,"text_map_units":bound.text_map.len(),"text_binding_present":bound.text_binding_digest.is_some(),"diagnostics":facts.diagnostics}));
        let raw = std::fs::read(workspace.join(&facts.version.path))?;
        let text = std::str::from_utf8(&raw)
            .ok()
            .map(|s| s.trim_start_matches('\u{feff}'))
            .filter(|s| {
                zg_storage::sha256_hex(s.as_bytes()) == facts.version.decoded_digest
                    && zg_storage::sha256_hex(&raw) == facts.version.raw_digest
            });
        for reference in &facts.references {
            let resolution = resolutions
                .get(reference.id.as_str())
                .ok_or("missing resolution record")?;
            let kind = serde_json::to_value(reference.kind)?
                .as_str()
                .ok_or("kind")?
                .to_owned();
            let status = serde_json::to_value(resolution.status)?
                .as_str()
                .ok_or("status")?
                .to_owned();
            *counts.entry(format!("{kind}/{status}")).or_default() += 1;
            let expression = text.and_then(|s| s.get(reference.span.start..reference.span.end));
            let shape = match &reference.lookup {
                Lookup::Name { parts } if parts.first().is_some_and(|p| p == "Self") => "self_path",
                Lookup::Name { parts } if parts.len() == 1 => "name",
                Lookup::Name { .. } => "qualified_path",
                Lookup::Dynamic if expression.is_some_and(|s| s.starts_with("self.")) => {
                    "dynamic_self_receiver"
                }
                Lookup::Dynamic if expression.is_some_and(|s| s.contains("::<")) => {
                    "dynamic_generic_syntax"
                }
                Lookup::Dynamic => "dynamic_other",
                _ => "import_or_document",
            };
            if reference.kind == RelationKind::Call {
                *reasons
                    .entry(format!("{status}/{}", resolution.reason))
                    .or_default() += 1;
                *shapes.entry(format!("{status}/{shape}")).or_default() += 1;
            }
            references.push(serde_json::json!({"source":facts.version.path,"language":facts.language,"id":reference.id,"owner":reference.owner,"kind":reference.kind,"span":reference.span,"lookup":reference.lookup,"status":resolution.status,"reason":resolution.reason,"targets":resolution.targets,"expression":expression,"shape_hint":shape}));
        }
    }
    let semantic = serde_json::json!({"digest":stable_digest(&catalog.semantic)?,"sources":catalog.semantic.sources.len(),"entities":catalog.semantic.entities.len(),"episodes":catalog.semantic.episodes.len(),"facts":catalog.semantic.facts.len(),"associations":catalog.semantic.associations.len(),"mentions":catalog.semantic.mentions.len(),"deferred_mentions":catalog.semantic.deferred_mentions.len()});
    let result = serde_json::json!({"kind":"selected_candidate_maintenance_audit","not_public_query_path":true,"proof_verified":true,"graph_build_id":proof.graph_build_id,"receipt_id":proof.receipt_id,"facts_digest":proof.facts_digest,"catalog_digest":stable_digest(&catalog)?,"source_projection_digest":projection_digest,"semantic":semantic,"sources":source_states,"counts":counts,"call_reasons":reasons,"call_shape_hints":shapes,"shape_note":"hints classify verified UTF-8 source slices; they are not type inference or an accuracy oracle","references":references,"elapsed_micros":started.elapsed().as_micros(),"source_languages":languages});
    std::fs::write(output, serde_json::to_vec(&result)?)?;
    println!("{}", serde_json::to_string(&result["counts"])?);
    Ok(())
}
