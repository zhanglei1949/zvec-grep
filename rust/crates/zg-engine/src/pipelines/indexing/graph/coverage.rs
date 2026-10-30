//! Coverage is source processing, not a count of rows or resolved targets.
use super::{check_context, roots};
use crate::{
    EngineResult,
    api::index::graph::{GraphDomainResult, GraphReady, GraphSourceCoverage, GraphSourceState},
    domain::{FileFormat, FileRecord},
    graph_facts::module_roots::{ModuleMapping, ModuleRoots},
    workspace::{graph_policy::GraphPolicy, manifest::WorkspaceManifest},
};
use std::collections::{BTreeMap, BTreeSet};
use zg_knowledge::{
    processing::ProcessingContext,
    relations::{
        Language,
        bound::{BoundFileFacts, FactCatalog},
    },
};

fn source_state(
    file: &FileRecord,
    language: Language,
    roots: &ModuleRoots,
    bound: Option<&BoundFileFacts>,
    resolved: &BTreeSet<(&str, &str)>,
) -> EngineResult<GraphSourceCoverage> {
    let source = file.relative_path.to_path_buf().to_str().map(str::to_owned);
    let facts = bound.and_then(|b| b.facts.as_ref());
    let diagnostics = facts.map_or_else(Vec::new, |facts| {
        facts.diagnostics.iter().map(|d| d.code.clone()).collect()
    });
    let state = if !file.index_status.is_indexed() {
        GraphSourceState::NotIndexed
    } else if let Some(path) = source.as_deref().filter(|path| {
        !path.contains(['\\', ':', '\0']) && !path.split('/').any(|c| matches!(c, "" | "." | ".."))
    }) {
        let mapping = if language == Language::Markdown {
            None
        } else {
            Some(roots.map(path, language)?)
        };
        match mapping {
            Some(ModuleMapping::Missing) => GraphSourceState::NoModuleRoot,
            Some(ModuleMapping::LanguageMismatch) => GraphSourceState::ModuleLanguageMismatch,
            _ => match facts {
                None => GraphSourceState::FactsMissing,
                Some(facts) if !facts.complete => GraphSourceState::IncompleteFacts,
                Some(facts)
                    if facts
                        .references
                        .iter()
                        .any(|reference| !resolved.contains(&(path, reference.id.as_str()))) =>
                {
                    GraphSourceState::ResolutionPending
                }
                Some(_) => GraphSourceState::Ready,
            },
        }
    } else {
        GraphSourceState::UnsupportedPath
    };
    Ok(GraphSourceCoverage {
        file_id: file.id.get(),
        source,
        state,
        diagnostics,
    })
}

pub(super) fn summarize(
    manifest: &WorkspaceManifest,
    policy: &GraphPolicy,
    files: &[FileRecord],
    catalog: &FactCatalog,
    context: &ProcessingContext,
) -> EngineResult<BTreeMap<String, GraphDomainResult>> {
    let roots = roots(&policy.configuration)?;
    let by_source = catalog
        .files
        .iter()
        .map(|f| (f.record.file_id, f))
        .collect::<BTreeMap<_, _>>();
    let resolved = catalog
        .resolutions
        .iter()
        .filter(|r| r.reason != "pending")
        .map(|r| (r.file_path.as_str(), r.reference_id.as_str()))
        .collect::<BTreeSet<_>>();
    let mut domains = BTreeMap::new();
    for (name, enabled, is_doc) in [
        ("code", policy.configuration.scope.code(), false),
        ("documents", policy.configuration.scope.documents(), true),
    ] {
        let mut result = GraphDomainResult {
            ready: GraphReady::Unsupported,
            coverage: 0,
            expected_coverage: enabled.then_some(0),
            sources: Vec::new(),
            node_count: 0,
            reference_count: 0,
        };
        if enabled {
            for file in files {
                check_context(context)?;
                let formats =
                    FileFormat::from_path(&manifest.workspace.source_path(&file.relative_path))?;
                let language = if formats.contains(&FileFormat::Markdown) {
                    Some(Language::Markdown)
                } else if formats.contains(&FileFormat::Rust) {
                    Some(Language::Rust)
                } else if formats.contains(&FileFormat::Python) {
                    Some(Language::Python)
                } else {
                    None
                };
                let Some(language) = language.filter(|l| (*l == Language::Markdown) == is_doc)
                else {
                    continue;
                };
                let bound = by_source.get(&file.id.get()).copied();
                let source = source_state(file, language, &roots, bound, &resolved)?;
                if source.state == GraphSourceState::Ready {
                    result.coverage += 1;
                }
                if let Some(facts) = bound.and_then(|b| b.facts.as_ref()) {
                    result.node_count += facts.nodes.len() as u64;
                    result.reference_count += facts.references.len() as u64;
                }
                result.sources.push(source);
            }
            result.sources.sort_by_key(|s| s.file_id);
            result.expected_coverage = Some(result.sources.len() as u64);
            result.ready = if result.coverage == result.sources.len() as u64 {
                GraphReady::Ready
            } else if !is_doc && policy.configuration.module_roots.is_empty() {
                GraphReady::NotReady("no_module_roots".into())
            } else {
                GraphReady::NotReady("incomplete_coverage".into())
            };
        }
        domains.insert(name.into(), result);
    }
    Ok(domains)
}

#[cfg(test)]
#[path = "coverage_tests.rs"]
mod tests;
