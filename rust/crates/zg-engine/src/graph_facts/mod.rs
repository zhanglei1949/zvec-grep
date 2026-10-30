//! Experimental G0/G1 inspection, enabled with `knowledge-graph`.
//! No files, model, zvec collection or graph database are opened by this API.
//! The caller supplies the selected-file inventory; this is NOT published query
//! evidence and must not bypass workspace selection in a future G2 integration.
pub mod binding;
pub(crate) mod code;
pub mod module_roots;
pub(crate) mod text_binding;

use serde::{Deserialize, Serialize};
use zg_knowledge::{
    processing::ProcessingContext,
    relations::{CodeModule, FileFacts, Language, ResolvedReference, SourceVersion, Span},
};

use crate::{
    EngineError, EngineResult,
    domain::{FileFormat, Range, SourcePath},
    extraction::{self, TextSource},
    utils::{decode_index_text, sha256_hex},
};

pub use zg_knowledge::relations;

/// Module identity is supplied by the host, never guessed from unrelated roots.
#[derive(Clone, Debug)]
pub struct FactInput {
    pub path: String,
    pub language: Language,
    pub bytes: Vec<u8>,
    pub module: Option<CodeModule>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FactInspection {
    pub files: Vec<FileFacts>,
    pub references: Vec<ResolvedReference>,
}

/// Inspects selected immutable inputs synchronously under caller budgets.
/// Run inside the host's bounded blocking executor when used from async code.
/// Coordinates exactly match the existing indexing decoder, including Python
/// encoding cookies and BOM removal. No persistent capability is implied.
///
/// # Errors
/// Returns validation, cancellation, deadline or budget errors without source text.
pub fn inspect_sources(
    generation: &str,
    inputs: &[FactInput],
    context: &ProcessingContext,
) -> EngineResult<FactInspection> {
    let total = inputs
        .iter()
        .try_fold(0usize, |n, i| n.checked_add(i.bytes.len()))
        .ok_or_else(|| EngineError::invalid_argument("graph input size overflow"))?;
    context.check_input(total).map_err(map_error)?;
    let mut paths = std::collections::HashSet::new();
    let mut files = Vec::new();
    for input in inputs {
        if !paths.insert(&input.path) {
            return Err(EngineError::invalid_argument("duplicate graph source path"));
        }
        let format = match input.language {
            Language::Rust => FileFormat::Rust,
            Language::Python => FileFormat::Python,
            Language::Markdown => FileFormat::Markdown,
        };
        let text = decode_index_text(&[format], &input.bytes)
            .ok_or_else(|| EngineError::invalid_argument("cannot decode graph input"))?;
        let version = SourceVersion {
            generation: generation.into(),
            path: input.path.clone(),
            raw_digest: sha256_hex(&input.bytes),
            decoded_digest: sha256_hex(text.as_bytes()),
        };
        let mut facts = if input.language == Language::Markdown {
            if input.module.is_some() {
                return Err(EngineError::invalid_argument("Markdown has no code module"));
            }
            relations::extract_markdown(version, &text, context).map_err(map_error)?
        } else {
            FileFacts::new(version, input.language, input.module.clone(), &text)
                .map_err(map_error)?
        };
        let source = TextSource {
            relative_path: SourcePath::new(&input.path)?,
            formats: vec![format],
            text: text.into_owned(),
        };
        let entities = if input.language == Language::Markdown {
            extraction::extract_for_indexing(&source, extraction::ChunkOptions::default())?
        } else {
            extraction::code::extract_with_graph(
                &source,
                extraction::ChunkOptions::default(),
                &mut facts,
                context,
            )?
        };
        bind_entity_indices(&mut facts, &entities);
        facts.validate(&source.text, context).map_err(map_error)?;
        files.push(facts);
        context
            .check_output(
                files
                    .iter()
                    .map(|f| {
                        f.nodes.len()
                            + f.references.len()
                            + f.bindings.len()
                            + f.scopes.len()
                            + f.anchors.len()
                            + f.diagnostics.len()
                    })
                    .sum(),
            )
            .map_err(map_error)?;
    }
    let mut references = relations::resolve_code(&files, context).map_err(map_error)?;
    references.extend(
        relations::resolve_documents(
            &files,
            &inputs.iter().map(|i| i.path.clone()).collect::<Vec<_>>(),
            context,
        )
        .map_err(map_error)?,
    );
    context.check_output(references.len()).map_err(map_error)?;
    Ok(FactInspection { files, references })
}

pub(crate) fn bind_entity_indices(facts: &mut FileFacts, entities: &[extraction::ExtractedEntity]) {
    // G2 binds these same-extraction positions to persisted EntityIds. Uncovered
    // evidence stays explicit; no new snapshot/body storage is created here.
    let covering = |span| {
        entities
            .iter()
            .enumerate()
            .filter_map(|(i, e)| match e.source_range {
                Range::Text(r)
                    if (Span {
                        start: r.start_byte_offset(),
                        end: r.end_byte_offset(),
                    })
                    .contains(span) =>
                {
                    Some(i)
                }
                _ => None,
            })
            .collect()
    };
    for node in &mut facts.nodes {
        node.entity_indices = covering(node.span);
    }
    for reference in &mut facts.references {
        reference.entity_indices = covering(reference.span);
    }
}

#[expect(
    clippy::needless_pass_by_value,
    reason = "map_err consumes the provider error and strips provider details"
)]
pub(crate) fn map_error(error: zg_knowledge::Error) -> EngineError {
    // Keep source text/provider details out of the host error surface.
    match error {
        zg_knowledge::Error::Cancelled => EngineError::cancelled("graph inspection cancelled"),
        zg_knowledge::Error::Deadline => {
            EngineError::deadline_exceeded("graph inspection deadline exceeded")
        }
        zg_knowledge::Error::ResourceExhausted => {
            EngineError::invalid_argument("graph inspection budget exhausted")
        }
        _ => EngineError::invalid_argument("invalid graph fact input or output"),
    }
}
