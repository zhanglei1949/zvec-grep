//! Model-free extraction diagnostics for a frozen inventory. Never publishes a view.
use std::{collections::BTreeMap, path::PathBuf};
use zg_engine::api::context::options::FileFormat;
use zg_engine::graph_facts::{
    FactInput,
    binding::{BindingOptions, BoundInput, inspect_bound_sources},
    module_roots::{ModuleMapping, ModuleRoot, ModuleRoots},
    relations::Language,
};
use zg_knowledge::processing::{ProcessingContext, stable_digest};
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args_os().skip(1);
    let root = PathBuf::from(args.next().ok_or("ROOT required")?).canonicalize()?;
    let manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(args.next().ok_or("MANIFEST required")?)?)?;
    let configuration: serde_json::Value = serde_json::from_slice(&std::fs::read(
        args.next().ok_or("GRAPH_OPTIONS required")?,
    )?)?;
    let output = PathBuf::from(args.next().ok_or("OUTPUT required")?);
    if args.next().is_some() {
        return Err("usage: ROOT MANIFEST GRAPH_OPTIONS OUTPUT".into());
    }
    let roots = ModuleRoots::new(serde_json::from_value::<Vec<ModuleRoot>>(
        configuration["module_roots"].clone(),
    )?)?;
    roots.validate_workspace(&root)?;
    let context = ProcessingContext {
        max_input_bytes: 128 << 20,
        max_output_items: 30_000_000,
        deadline: None,
        cancellation: tokio_util::sync::CancellationToken::new(),
    };
    let mut inputs = Vec::new();
    let mut sources = Vec::new();
    for record in manifest["records"].as_array().ok_or("records required")? {
        let path = record["path"].as_str().ok_or("path")?;
        let extension = std::path::Path::new(path)
            .extension()
            .and_then(|e| e.to_str());
        let language = match extension.and_then(FileFormat::parse) {
            Some(FileFormat::Rust) => Language::Rust,
            Some(FileFormat::Python) => Language::Python,
            _ => continue,
        };
        let bytes = std::fs::read(root.join(path))?;
        if Some(zg_storage::sha256_hex(&bytes).as_str()) != record["sha256"].as_str() {
            return Err(format!("frozen source changed: {path}").into());
        }
        let mapping = roots.map(path, language)?;
        let status = match &mapping {
            ModuleMapping::Mapped(_) => "pending",
            ModuleMapping::Missing => "no_module_root",
            ModuleMapping::LanguageMismatch => "module_language_mismatch",
        };
        sources.push(serde_json::json!({"path":path,"language":language,"state":status,"hidden_path":path.split('/').any(|p|p.starts_with('.')),"diagnostics":[]}));
        if let ModuleMapping::Mapped(module) = mapping {
            inputs.push(BoundInput {
                file_id: u32::try_from(sources.len())?,
                input: FactInput {
                    path: path.into(),
                    language,
                    bytes,
                    module: Some(module),
                },
            });
        }
    }
    let facts = inspect_bound_sources(
        "g3.2-coverage",
        0,
        &inputs,
        BindingOptions::default(),
        &context,
    )?;
    for file in facts {
        let source = sources
            .iter_mut()
            .find(|s| s["path"] == file.record.path)
            .ok_or("source missing")?;
        let facts = file.facts.ok_or("mapped source lacks facts")?;
        source["state"] = if facts.complete {
            "complete"
        } else {
            "incomplete_facts"
        }
        .into();
        source["diagnostics"] = serde_json::to_value(&facts.diagnostics)?;
    }
    let Counts { states, causes } = counts(&sources)?;
    let report = serde_json::json!({"kind":"frozen_inventory_extraction_diagnostics","published":false,"search_ready":false,"text_index_verified":false,"module_roots_fingerprint":roots.fingerprint(),"manifest_fingerprint":stable_digest(&manifest)?,"sources":sources,"states":states,"diagnostic_source_counts":causes});
    std::fs::write(output, serde_json::to_vec_pretty(&report)?)?;
    println!(
        "{}",
        serde_json::to_string(
            &serde_json::json!({"states":states,"diagnostic_source_counts":causes})
        )?
    );
    Ok(())
}

struct Counts {
    states: BTreeMap<String, usize>,
    causes: BTreeMap<String, usize>,
}

fn counts(sources: &[serde_json::Value]) -> Result<Counts, Box<dyn std::error::Error>> {
    let mut states = BTreeMap::<String, usize>::new();
    let mut causes = BTreeMap::<String, usize>::new();
    for source in sources {
        *states
            .entry(source["state"].as_str().ok_or("state")?.into())
            .or_default() += 1;
        let codes = source["diagnostics"]
            .as_array()
            .ok_or("diagnostics")?
            .iter()
            .filter_map(|d| d["code"].as_str())
            .collect::<std::collections::BTreeSet<_>>();
        for code in codes {
            *causes.entry(code.into()).or_default() += 1;
        }
    }
    Ok(Counts { states, causes })
}
