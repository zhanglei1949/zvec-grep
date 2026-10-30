//! Same-extraction bridge. The capture sink observes successful text writes;
//! it neither publishes a graph nor changes ordinary indexing configuration.
use super::{
    FactInput,
    module_roots::{ModuleMapping, ModuleRoots},
};
use crate::{
    EngineError, EngineResult,
    domain::{Entity, FileFormat, FileRecord},
    extraction::{self, ChunkOptions, ExtractedEntity, TextSource},
    utils::sha256_hex,
};
use std::{
    collections::BTreeMap,
    sync::{Mutex, PoisonError},
};
use zg_knowledge::{
    processing::ProcessingContext,
    relations::{
        self, FileFacts, Language, SourceVersion,
        bound::{BoundFileFacts, FileRecord as BoundRecord, IndexStatus},
    },
};

struct ExtractionOutcome {
    entities: Vec<ExtractedEntity>,
    facts: EngineResult<Option<FileFacts>>,
}

pub(crate) fn extract(
    source: &TextSource,
    options: ChunkOptions,
    raw_digest: &str,
    generation: &str,
    module: Option<relations::CodeModule>,
    context: &ProcessingContext,
) -> EngineResult<(Vec<ExtractedEntity>, Option<FileFacts>)> {
    let outcome = extract_outcome(source, options, raw_digest, generation, module, context)?;
    Ok((outcome.entities, outcome.facts?))
}

fn extract_outcome(
    source: &TextSource,
    options: ChunkOptions,
    raw_digest: &str,
    generation: &str,
    module: Option<relations::CodeModule>,
    context: &ProcessingContext,
) -> EngineResult<ExtractionOutcome> {
    let language = if source.formats.contains(&FileFormat::Rust) {
        Some(Language::Rust)
    } else if source.formats.contains(&FileFormat::Python) {
        Some(Language::Python)
    } else if source.formats.contains(&FileFormat::Markdown) {
        Some(Language::Markdown)
    } else {
        None
    };
    let Some(language) = language else {
        return Ok(ExtractionOutcome {
            entities: extraction::extract_for_indexing(source, options)?,
            facts: Ok(None),
        });
    };
    if language != Language::Markdown && module.is_none() {
        return Ok(ExtractionOutcome {
            entities: extraction::extract_for_indexing(source, options)?,
            facts: Ok(None),
        });
    }
    let initialized = (|| {
        if language == Language::Markdown && module.is_some() {
            return Err(EngineError::invalid_argument("Markdown has no code module"));
        }
        let path = source
            .relative_path
            .to_path_buf()
            .into_os_string()
            .into_string()
            .map_err(|_| EngineError::unsupported("graph path is not UTF-8"))?;
        let version = SourceVersion {
            generation: generation.into(),
            path,
            raw_digest: raw_digest.into(),
            decoded_digest: sha256_hex(source.text.as_bytes()),
        };
        if language == Language::Markdown {
            relations::extract_markdown(version, &source.text, context).map_err(super::map_error)
        } else {
            FileFacts::new(version, language, module, &source.text).map_err(super::map_error)
        }
    })();
    let mut facts = match initialized {
        Ok(f) => f,
        Err(error) => {
            return Ok(ExtractionOutcome {
                entities: extraction::extract_for_indexing(source, options)?,
                facts: Err(error),
            });
        }
    };
    let (entities, collected) = if language == Language::Markdown {
        (extraction::extract_for_indexing(source, options)?, Ok(()))
    } else {
        extraction::code::extract_with_graph_outcome(source, options, &mut facts, context)?
    };
    let facts = collected.and_then(|()| {
        for entity in &entities {
            let crate::domain::Range::Text(range) = entity.source_range else { continue; };
            if !matches!(&entity.content, crate::domain::Content::Text(body) if source.text.get(range.start_byte_offset()..range.end_byte_offset()) == Some(body.as_str())) {
                return Err(EngineError::storage_failure("extracted text differs from source"));
            }
        }
        super::bind_entity_indices(&mut facts, &entities);
        facts
            .validate(&source.text, context)
            .map_err(super::map_error)?;
        Ok(Some(facts))
    });
    Ok(ExtractionOutcome { entities, facts })
}

/// Diagnostic input; `file_id` must be the ID allocated by the host, not a row ID.
#[derive(Clone, Debug)]
pub struct BoundInput {
    pub input: FactInput,
    pub file_id: u32,
}
#[derive(Clone, Copy, Debug, Default)]
pub struct BindingOptions {
    pub max_chunk_chars: Option<usize>,
    pub chunk_overlap_chars: Option<usize>,
}

/// Model-free candidate inspection using the very same extraction/EntityId
/// allocation functions as the indexing pipeline. This does NOT write text.
///
/// # Errors
/// Rejects duplicate identities, invalid module/encoding/positions, invalid
/// chunk limits, cancellation, expired deadlines and exhausted budgets.
pub fn inspect_bound_sources(
    generation: &str,
    text_revision: u64,
    inputs: &[BoundInput],
    options: BindingOptions,
    context: &ProcessingContext,
) -> EngineResult<Vec<BoundFileFacts>> {
    context
        .check_input(
            inputs
                .iter()
                .try_fold(0usize, |n, i| n.checked_add(i.input.bytes.len()))
                .ok_or_else(|| EngineError::invalid_argument("graph input overflow"))?,
        )
        .map_err(super::map_error)?;
    let mut paths = std::collections::BTreeSet::new();
    let mut file_ids = std::collections::BTreeSet::new();
    inputs
        .iter()
        .map(|item| {
            let input = &item.input;
            if !paths.insert(&input.path) || !file_ids.insert(item.file_id) {
                return Err(EngineError::invalid_argument(
                    "duplicate graph source identity",
                ));
            }
            let format = match input.language {
                Language::Rust => FileFormat::Rust,
                Language::Python => FileFormat::Python,
                Language::Markdown => FileFormat::Markdown,
            };
            let text = crate::utils::decode_index_text(&[format], &input.bytes)
                .ok_or_else(|| EngineError::invalid_argument("cannot decode graph input"))?;
            let source = TextSource {
                relative_path: crate::domain::SourcePath::new(&input.path)?,
                formats: vec![format],
                text: text.into_owned(),
            };
            let hash = sha256_hex(&input.bytes);
            let (entities, facts) = extract(
                &source,
                ChunkOptions {
                    max_chunk_chars: options.max_chunk_chars,
                    chunk_overlap_chars: options.chunk_overlap_chars,
                },
                &hash,
                generation,
                input.module.clone(),
                context,
            )?;
            let entities =
                extraction::bind_entities(crate::domain::FileId::new(item.file_id), entities)?;
            let ids = entities
                .iter()
                .map(|e| e.id.as_str().to_owned())
                .collect::<Vec<_>>();
            let record = BoundRecord {
                path: input.path.clone(),
                file_id: item.file_id,
                index_status: IndexStatus::NotIndexed,
                content_hash: Some(hash),
                size_bytes: input.bytes.len() as u64,
                text_revision,
                storage_generation: generation.into(),
                entity_count: 0,
                entity_ids_digest: String::new(),
            };
            BoundFileFacts::bind(
                facts
                    .ok_or_else(|| EngineError::invalid_argument("code module mapping required"))?,
                record,
                &ids,
                context,
            )
            .map_err(super::map_error)
        })
        .collect()
}

pub(crate) struct FactCapture {
    pub generation: String,
    pub text_revision: u64,
    pub roots: ModuleRoots,
    pub context: ProcessingContext,
    files: Mutex<BTreeMap<String, BoundFileFacts>>,
    diagnostics: Mutex<BTreeMap<String, String>>,
    // Full final host inventory, including unchanged/unsupported/failed files.
    // Missing same-run bindings remain explicit; G2.2 must reuse verified facts
    // or fill them before publishing, never silently treating this as complete.
    inventory: Mutex<Vec<FileRecord>>,
}
impl FactCapture {
    #[cfg(any(test, feature = "knowledge-graph-graphdb"))]
    pub(crate) fn new(
        generation: String,
        text_revision: u64,
        roots: ModuleRoots,
        context: ProcessingContext,
    ) -> std::sync::Arc<Self> {
        std::sync::Arc::new(Self {
            generation,
            text_revision,
            roots,
            context,
            files: Mutex::default(),
            diagnostics: Mutex::default(),
            inventory: Mutex::default(),
        })
    }
    pub(crate) fn extract(
        &self,
        source: &TextSource,
        options: ChunkOptions,
        raw_digest: &str,
    ) -> EngineResult<(Vec<ExtractedEntity>, Option<FileFacts>)> {
        let Some(path) = source
            .relative_path
            .to_path_buf()
            .to_str()
            .map(str::to_owned)
        else {
            // Ordinary indexing supports native paths. Do not fail text indexing.
            return Ok((extraction::extract_for_indexing(source, options)?, None));
        };
        if !supported_path(&path) {
            self.diagnostics
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .insert(path, "unsupported_graph_path".into());
            return Ok((extraction::extract_for_indexing(source, options)?, None));
        }
        let language = if source.formats.contains(&FileFormat::Rust) {
            Some(Language::Rust)
        } else if source.formats.contains(&FileFormat::Python) {
            Some(Language::Python)
        } else {
            None
        };
        let module = if let Some(language) = language {
            match self.roots.map(&path, language)? {
                ModuleMapping::Mapped(m) => Some(m),
                mapping => {
                    self.diagnostics
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .insert(
                            path.clone(),
                            if mapping == ModuleMapping::Missing {
                                "no_module_root"
                            } else {
                                "module_language_mismatch"
                            }
                            .into(),
                        );
                    None
                }
            }
        } else {
            None
        };
        let outcome = extract_outcome(
            source,
            options,
            raw_digest,
            &self.generation,
            module,
            &self.context,
        )?;
        match outcome.facts {
            Ok(facts) => Ok((outcome.entities, facts)),
            Err(error)
                if matches!(
                    error.code(),
                    EngineError::CANCELLED | EngineError::DEADLINE_EXCEEDED
                ) =>
            {
                Err(error)
            }
            Err(_) => {
                self.diagnostics
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .insert(path, "graph_fact_processing_failed".into());
                Ok((outcome.entities, None))
            }
        }
    }
    pub(crate) fn bind(
        &self,
        file: &FileRecord,
        entities: &[Entity],
        facts: Option<FileFacts>,
    ) -> EngineResult<Option<BoundFileFacts>> {
        let Some(path) = file.relative_path.to_path_buf().to_str().map(str::to_owned) else {
            self.diagnostics
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .insert(format!("file-id:{}", file.id), "non_utf8_graph_path".into());
            return Ok(None);
        };
        if !supported_path(&path) {
            return Ok(None);
        }
        let ids = entities
            .iter()
            .map(|e| e.id.as_str().to_owned())
            .collect::<Vec<_>>();
        let record = BoundRecord {
            path,
            file_id: file.id.get(),
            index_status: IndexStatus::NotIndexed,
            content_hash: file.snapshot.content_hash.clone(),
            size_bytes: file.snapshot.size_bytes,
            text_revision: self.text_revision,
            storage_generation: self.generation.clone(),
            entity_count: u64::try_from(ids.len())
                .map_err(|_| EngineError::invalid_argument("entity count overflow"))?,
            entity_ids_digest: zg_knowledge::relations::bound::entity_ids_digest(&ids)
                .map_err(super::map_error)?,
        };
        match facts {
            Some(facts) => BoundFileFacts::bind(facts, record, &ids, &self.context),
            None => BoundFileFacts::inventory_only(record, &self.context),
        }
        .map_err(super::map_error)
        .and_then(|mut bound| {
            if bound.facts.is_some() {
                bound.text_map = super::text_binding::maps(&bound.record.path, entities)?;
                bound.text_binding_digest = Some(super::text_binding::digest(entities)?);
            }
            Ok(Some(bound))
        })
    }
    pub(crate) fn committed(&self, mut facts: BoundFileFacts) {
        // Only called after replace_file succeeds. These are still unselected
        // candidates; final inventory/checkpoint validation belongs to G2.2.
        facts.record.index_status = IndexStatus::Indexed;
        let mut files = self.files.lock().unwrap_or_else(PoisonError::into_inner);
        let cost = |f: &BoundFileFacts| {
            let structure = f.facts.as_ref().map_or(0, |f| {
                f.nodes
                    .len()
                    .saturating_add(f.scopes.len())
                    .saturating_add(f.bindings.len())
                    .saturating_add(f.references.len())
                    .saturating_add(f.anchors.len())
                    .saturating_add(f.diagnostics.len())
            });
            structure.saturating_add(1).saturating_add(
                f.node_entities
                    .iter()
                    .chain(&f.reference_entities)
                    .map(Vec::len)
                    .sum::<usize>(),
            )
        };
        let retained = files
            .values()
            .filter(|f| f.record.path != facts.record.path)
            .fold(0usize, |n, f| n.saturating_add(cost(f)));
        if retained.saturating_add(cost(&facts)) > self.context.max_output_items {
            self.diagnostics
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .insert(facts.record.path, "graph_capture_budget".into());
            return;
        }
        files.insert(facts.record.path.clone(), facts);
    }
    pub(crate) fn finish(&self, inventory: Vec<FileRecord>) {
        self.files
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .retain(|_, bound| {
                inventory.iter().any(|f| {
                    f.id.get() == bound.record.file_id
                        && f.index_status.is_indexed()
                        && f.snapshot.content_hash == bound.record.content_hash
                        && f.snapshot.size_bytes == bound.record.size_bytes
                        && f.index_status.entity_count() == bound.record.entity_count
                })
            });
        *self
            .inventory
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = inventory;
    }
    #[cfg(any(test, feature = "knowledge-graph-graphdb"))]
    pub(crate) fn collected(&self) -> Vec<BoundFileFacts> {
        self.files
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .values()
            .cloned()
            .collect()
    }
}

fn supported_path(path: &str) -> bool {
    !path.is_empty()
        && !path.contains(['\\', ':', '\0'])
        && !path.split('/').any(|p| matches!(p, "" | "." | ".."))
}
