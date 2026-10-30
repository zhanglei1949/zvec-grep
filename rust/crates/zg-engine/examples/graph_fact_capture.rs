//! Model-free L1 capture for the G2.1 native size probe. No text index is built
//! and the synthesized eligibility fixture MUST NOT be selected or served.
use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
    process::Command,
};
use zg_engine::graph_facts::{
    FactInput,
    binding::{BindingOptions, BoundInput, inspect_bound_sources},
    module_roots::{ModuleMapping, ModuleRoot, ModuleRoots, PackageLayout},
    relations::{
        Language,
        bound::{FactCatalog, IndexStatus, synthetic_id},
    },
};
use zg_knowledge::processing::{ProcessingContext, stable_digest};

fn ids(
    files: &[zg_knowledge::relations::bound::BoundFileFacts],
) -> Result<BTreeMap<String, Vec<String>>, Box<dyn std::error::Error>> {
    files
        .iter()
        .map(|f| {
            let facts = f.facts.as_ref().ok_or("facts")?;
            let mut ids = Vec::new();
            for (kind, n) in [
                ("scope", facts.scopes.len()),
                ("binding", facts.bindings.len()),
                ("diagnostic", facts.diagnostics.len()),
            ] {
                for ordinal in 0..n {
                    ids.push(synthetic_id(&f.record.path, kind, ordinal)?);
                }
            }
            Ok((f.record.path.clone(), ids))
        })
        .collect()
}
#[expect(
    clippy::too_many_lines,
    reason = "keep the capture, repeat and configuration-change measurement protocol in order"
)]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args_os().skip(1);
    let root = PathBuf::from(args.next().ok_or("repository root required")?).canonicalize()?;
    let output = PathBuf::from(args.next().ok_or("output directory required")?);
    std::fs::create_dir_all(&output)?;
    let selected = Command::new("git")
        .args([
            "ls-files",
            "--cached",
            "--others",
            "--exclude-standard",
            "-z",
            "--",
            "rust/crates",
        ])
        .current_dir(&root)
        .output()?;
    if !selected.status.success() {
        return Err("git inventory failed".into());
    }
    let paths = selected
        .stdout
        .split(|b| *b == 0)
        .filter(|p| !p.is_empty())
        .map(std::str::from_utf8)
        .collect::<Result<BTreeSet<_>, _>>()?;
    #[expect(
        clippy::case_sensitive_file_extension_comparisons,
        reason = "the fixed L1 protocol selects literal lowercase Rust source extensions"
    )]
    let paths = paths
        .into_iter()
        .filter(|p| p.ends_with(".rs"))
        .collect::<Vec<_>>();
    let mut explicit_roots = BTreeMap::new();
    for path in &paths {
        let mut parent = root.join(path).parent().ok_or("parent")?.to_path_buf();
        while !parent.join("Cargo.toml").is_file() {
            if !parent.pop() || !parent.starts_with(&root) {
                return Err("source outside a crate".into());
            }
        }
        let relative = parent.strip_prefix(&root)?.to_path_buf();
        let namespace = relative.to_str().ok_or("UTF-8 root")?.to_owned();
        explicit_roots.insert(
            namespace.clone(),
            ModuleRoot {
                root: relative,
                language: Language::Rust,
                namespace,
                module_prefix: Vec::new(),
                layout: PackageLayout::Flat,
            },
        );
    }
    let roots = ModuleRoots::new(explicit_roots.into_values().collect())?;
    let mut inputs = Vec::new();
    let mut manifest = Vec::new();
    for (id, path) in paths.iter().enumerate() {
        let bytes = std::fs::read(root.join(path))?;
        let ModuleMapping::Mapped(module) = roots.map(path, Language::Rust)? else {
            return Err("missing explicit L1 mapping".into());
        };
        manifest.push(serde_json::json!({"path":path,"sha256":zg_storage::sha256_hex(&bytes),"bytes":bytes.len(),"module":module}));
        inputs.push(BoundInput {
            file_id: u32::try_from(id + 1)?,
            input: FactInput {
                path: (*path).into(),
                language: Language::Rust,
                bytes,
                module: Some(module),
            },
        });
    }
    let context = ProcessingContext {
        max_input_bytes: 64 << 20,
        max_output_items: 30_000_000,
        deadline: None,
        cancellation: tokio_util::sync::CancellationToken::new(),
    };
    let normal = inspect_bound_sources("g2.1-l1", 1, &inputs, BindingOptions::default(), &context)?;
    let repeat = inspect_bound_sources("g2.1-l1", 1, &inputs, BindingOptions::default(), &context)?;
    if normal != repeat {
        return Err("repeated capture changed".into());
    }
    let chunked = inspect_bound_sources(
        "g2.1-l1",
        1,
        &inputs,
        BindingOptions {
            max_chunk_chars: Some(256),
            chunk_overlap_chars: Some(32),
        },
        &context,
    )?;
    if ids(&normal)? != ids(&chunked)? {
        return Err("synthetic IDs changed with chunk configuration".into());
    }
    if normal.iter().zip(&chunked).any(|(a, b)| a.facts != b.facts) {
        return Err("structural facts changed with chunk configuration".into());
    }
    let changed_bindings = normal
        .iter()
        .zip(&chunked)
        .filter(|(a, b)| a.record.entity_ids_digest != b.record.entity_ids_digest)
        .count();
    let synthetic_digest = stable_digest(&ids(&normal)?)?;
    // A size fixture models successful host eligibility to exercise resolution;
    // the real indexing/dual-store test independently verifies actual text IDs.
    let mut files = normal;
    for f in &mut files {
        f.record.index_status = IndexStatus::Indexed;
    }
    let catalog = FactCatalog::resolve(
        files,
        roots.fingerprint().into(),
        stable_digest(&manifest)?,
        &context,
    )?;
    std::fs::write(output.join("catalog.json"), serde_json::to_vec(&catalog)?)?;
    let summary = serde_json::json!({"selection":"git cached + nonignored untracked rust/crates/**/*.rs; no target/thirdparty, no model/text index","files":inputs.len(),"manifest":manifest,
        "same_input_equal":true,"chunk_structure_equal":true,"chunk_ids_equal":true,"synthetic_ids_digest":synthetic_digest,"changed_entity_bindings":changed_bindings,"catalog_digest":stable_digest(&catalog)?,"published":false,"search_ready":false});
    std::fs::write(
        output.join("capture.json"),
        serde_json::to_vec_pretty(&summary)?,
    )?;
    println!(
        "files={} changed_entity_bindings={} synthetic_ids_digest={synthetic_digest}",
        inputs.len(),
        changed_bindings
    );
    Ok(())
}
