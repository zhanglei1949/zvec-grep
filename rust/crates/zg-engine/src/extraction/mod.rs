//! Deterministic source extraction used by the indexing and lexical-enrichment paths.

mod chunking;
pub(crate) mod code;
mod image;
mod markdown;
mod service;
mod spi;
mod text;

// Extraction sources.
pub(crate) use spi::{ImageSource, Source, SourceKind, TextSource};

// Chunking options.
pub(crate) use spi::ChunkOptions;

// Indexing output.
pub(crate) use spi::{ExtractedEntity, ExtractedEntityFragment};

use crate::{
    EngineError,
    domain::{
        Content, Entity, EntityFragment, EntityId, EntityMetadata, FileFormat, FileId, FragmentId,
        TextRange,
    },
};

// Shared implementation helpers used by the format-specific extractors.
use service::{chunk_options_for_metadata, validate_formats};

#[cfg(test)]
use service::{test_content, test_metadata, test_source};

pub(crate) fn source_kind(formats: &[FileFormat]) -> Option<SourceKind> {
    service::source_kind(formats)
}

pub(crate) fn extract<'source>(
    source: impl Into<Source<'source>>,
    options: ChunkOptions,
) -> Result<Vec<ExtractedEntity>, EngineError> {
    service::extract(source, options)
}

pub(crate) fn extract_for_indexing<'source>(
    source: impl Into<Source<'source>>,
    options: ChunkOptions,
) -> Result<Vec<ExtractedEntity>, EngineError> {
    service::extract_for_indexing(source, options)
}

pub(crate) fn vector_content_for_fragment(
    content: &Content,
    metadata: Option<&EntityMetadata>,
    max_chars: Option<usize>,
) -> Vec<Content> {
    service::vector_content_for_fragment(content, metadata, max_chars)
}

pub(crate) fn bind_entities(
    file_id: FileId,
    extracted: Vec<ExtractedEntity>,
) -> Result<Vec<Entity>, EngineError> {
    extracted
        .into_iter()
        .map(|entity| {
            let id = EntityId::new(file_id, &entity.content, entity.source_range)?;
            let fragments = entity
                .fragments
                .into_iter()
                .enumerate()
                .map(|(ordinal, fragment)| {
                    let ordinal = u32::try_from(ordinal).map_err(|_| {
                        EngineError::invalid_argument("fragment ordinal exceeds u32 limits")
                    })?;
                    Ok(EntityFragment {
                        id: FragmentId::new(&id, ordinal),
                        range: fragment.range,
                    })
                })
                .collect::<Result<Vec<_>, EngineError>>()?;
            Ok(Entity {
                id,
                file_id,
                source_range: entity.source_range,
                content: entity.content,
                metadata: entity.metadata,
                fragments,
            })
        })
        .collect()
}
