use crate::{
    EngineError,
    domain::{Content, EntityMetadata, FileFormat, MarkdownMetadata, Range},
    utils::line_byte_offsets,
};

use super::{
    ChunkOptions, ExtractedEntity, TextRange, TextSource, chunk_options_for_metadata,
    chunking::text_fragments, text::extract_plain_text_entities, validate_formats,
};

const DEFAULT_MARKDOWN_CHUNK_CHARS: usize = 3_600;
const DEFAULT_MARKDOWN_CHUNK_OVERLAP_CHARS: usize = 540;

#[derive(Clone, Debug)]
struct Heading {
    level: usize,
    text: String,
    line_index: usize,
}

#[derive(Clone, Debug)]
struct Section {
    heading: Option<Heading>,
    start_index: usize,
    end_index: usize,
    breadcrumb: Vec<String>,
}

pub(super) fn extract(
    source: &TextSource,
    options: ChunkOptions,
) -> Result<Vec<ExtractedEntity>, EngineError> {
    if !source.formats.contains(&FileFormat::Markdown) {
        return Ok(Vec::new());
    }
    validate_formats(&source.formats)?;
    let (max_chars, overlap_chars) = resolve_options(options)?;
    let lines = source.text.split('\n').collect::<Vec<_>>();
    let line_offsets = line_byte_offsets(&source.text);
    let headings = scan_headings(&source.text, &line_offsets);
    if headings.is_empty() {
        return Ok(extract_plain_text_entities(
            source,
            max_chars,
            overlap_chars,
        ));
    }
    let entities = build_sections(&headings, &lines)
        .into_iter()
        .enumerate()
        .map(|(index, section)| {
            let metadata = markdown_metadata(&section);
            let (content_max, content_overlap) =
                chunk_options_for_metadata(max_chars, overlap_chars, Some(&metadata));
            let range = section_range(
                &lines,
                &line_offsets,
                section.start_index,
                section.end_index,
            );
            let content = crate::utils::slice_text(
                &source.text,
                range.start_byte_offset(),
                range.end_byte_offset(),
            )
            .expect("section range refers to source")
            .to_owned();
            ExtractedEntity {
                index,
                source_range: Range::Text(range),
                fragments: text_fragments(&content, content_max, content_overlap),
                content: Content::Text(content),
                metadata: Some(metadata),
            }
        })
        .collect::<Vec<_>>();
    if entities.is_empty() {
        Ok(extract_plain_text_entities(
            source,
            max_chars,
            overlap_chars,
        ))
    } else {
        Ok(entities)
    }
}

fn resolve_options(options: ChunkOptions) -> Result<(usize, usize), EngineError> {
    let max_chars = options
        .max_chunk_chars
        .unwrap_or(DEFAULT_MARKDOWN_CHUNK_CHARS);
    let overlap_chars = options
        .chunk_overlap_chars
        .unwrap_or(DEFAULT_MARKDOWN_CHUNK_OVERLAP_CHARS);
    if max_chars == 0 {
        return Err(EngineError::invalid_argument(
            "markdown extractor requires a positive integer chunk size",
        ));
    }
    if overlap_chars >= max_chars {
        return Err(EngineError::invalid_argument(
            "markdown extractor requires overlap to be smaller than chunk size",
        ));
    }
    Ok((max_chars, overlap_chars))
}

fn scan_headings(text: &str, line_offsets: &[usize]) -> Vec<Heading> {
    // Heading detection is shared with the knowledge graph via zg-markdown
    // (CommonMark); only the flat retrieval-section spanning below stays
    // engine-specific. Byte offsets map back to the engine's line-index model.
    zg_markdown::parse(text)
        .filter_map(|node| match node {
            zg_markdown::MdNode::Heading(heading) => {
                let line_index = line_index_of(heading.span.start, line_offsets);
                Some(Heading {
                    level: heading.level as usize,
                    text: heading.text,
                    line_index,
                })
            }
            zg_markdown::MdNode::Link(_) => None,
        })
        .collect()
}

fn line_index_of(byte_offset: usize, line_offsets: &[usize]) -> usize {
    line_offsets
        .partition_point(|&start| start <= byte_offset)
        .saturating_sub(1)
}

fn build_sections(headings: &[Heading], lines: &[&str]) -> Vec<Section> {
    let mut stack: Vec<&Heading> = Vec::new();
    let mut sections = Vec::new();
    let first = &headings[0];
    if first.line_index > 0 && !lines[..first.line_index].join("\n").trim().is_empty() {
        sections.push(Section {
            heading: None,
            start_index: 0,
            end_index: first.line_index - 1,
            breadcrumb: Vec::new(),
        });
    }

    for (index, heading) in headings.iter().enumerate() {
        while stack.last().is_some_and(|item| item.level >= heading.level) {
            stack.pop();
        }
        sections.push(Section {
            heading: Some(heading.clone()),
            start_index: heading.line_index,
            end_index: headings
                .get(index + 1)
                .map_or(lines.len() - 1, |next| next.line_index - 1),
            breadcrumb: stack.iter().map(|item| item.text.clone()).collect(),
        });
        stack.push(heading);
    }
    sections
}

fn section_range(
    lines: &[&str],
    line_offsets: &[usize],
    start_index: usize,
    end_index: usize,
) -> TextRange {
    TextRange::from_coordinates(
        line_offsets[start_index],
        line_offsets[end_index] + lines[end_index].len(),
        start_index + 1,
        end_index + 1,
        0,
        lines[end_index].len(),
    )
    .expect("section coordinates refer to source lines")
}

fn markdown_metadata(section: &Section) -> EntityMetadata {
    EntityMetadata::Markdown(MarkdownMetadata {
        heading: section.heading.as_ref().map(|heading| heading.text.clone()),
        level: section.heading.as_ref().map(|heading| heading.level),
        scope: (!section.breadcrumb.is_empty()).then(|| section.breadcrumb.join("::")),
    })
}

#[cfg(test)]
mod tests {
    use crate::domain::{Content, EntityMetadata, FileFormat, MarkdownMetadata, Range, TextRange};

    use super::super::{test_content, test_metadata};

    use super::super::{ChunkOptions, test_source};
    use super::extract;

    #[test]
    fn handles_heading_styles_fences_hierarchy_and_windows() {
        let source = test_source(
            FileFormat::Markdown,
            "README.md",
            &[
                "前言 😀",
                "",
                "# Parent #",
                "intro paragraph",
                "```md",
                "# Not a heading",
                "```",
                "## Child",
                "- item one",
                "- 项目 😀 with enough text to force another window",
                "Setext child",
                "------------",
                "body",
                "",
            ]
            .join("\r\n"),
        );
        let fragments = extract(
            &source,
            ChunkOptions {
                max_chunk_chars: Some(48),
                chunk_overlap_chars: Some(8),
            },
        )
        .expect("markdown extraction");
        // CommonMark: a setext underline after a list item is a lazy
        // continuation, not a heading, so "Setext child" folds into the Child
        // section. Entities are preamble + Parent + Child; the pre-CommonMark
        // line scanner over-detected "Setext child" as a fourth heading.
        assert_eq!(fragments.len(), 3);
        assert!(fragments.iter().any(|item| matches!(
            &test_metadata(item),
            Some(EntityMetadata::Markdown(MarkdownMetadata { heading: Some(heading), .. })) if heading == "Parent"
        )));
        assert!(fragments.iter().any(|item| matches!(
            &test_metadata(item),
            Some(EntityMetadata::Markdown(MarkdownMetadata {
                heading: Some(heading),
                scope: Some(scope),
                ..
            })) if heading == "Child" && scope == "Parent"
        )));
        assert!(!fragments.iter().any(|item| matches!(
            &test_metadata(item),
            Some(EntityMetadata::Markdown(MarkdownMetadata { heading: Some(heading), .. })) if heading == "Not a heading"
        )));
        assert!(fragments.iter().any(|entity| entity.fragments.len() > 1));
        assert_eq!(
            *fragments[0].source_range(),
            Range::Text(
                TextRange::from_coordinates(0, "前言 😀\r\n\r".len(), 1, 2, 0, 1)
                    .expect("preamble coordinates")
            )
        );

        assert_source_backed(&source, &fragments);
    }

    #[test]
    fn commonmark_detects_headings_the_old_line_scanner_missed() {
        // Regression cover for the P0 audit defects D1-D4: indented ATX,
        // blockquote headings, inline-code heading text and multi-line setext
        // are all detected via CommonMark (zg-markdown), unlike the old scanner.
        let source = test_source(
            FileFormat::Markdown,
            "README.md",
            "   # Indented ATX\n> # Quoted\n## Use `code` Here\nMulti\nLine\n===\n",
        );
        let entities = extract(&source, ChunkOptions::default()).expect("extract");
        let headings: Vec<(String, Option<usize>)> = entities
            .iter()
            .filter_map(|entity| match test_metadata(entity) {
                Some(EntityMetadata::Markdown(MarkdownMetadata {
                    heading: Some(heading),
                    level,
                    ..
                })) => Some((heading.clone(), *level)),
                _ => None,
            })
            .collect();
        assert_eq!(
            headings,
            vec![
                ("Indented ATX".to_owned(), Some(1)),
                ("Quoted".to_owned(), Some(1)),
                ("Use code Here".to_owned(), Some(2)),
                ("Multi Line".to_owned(), Some(1)),
            ]
        );
    }

    fn assert_source_backed(source: &super::TextSource, fragments: &[super::ExtractedEntity]) {
        for (index, entity) in fragments.iter().enumerate() {
            assert_eq!(entity.index, index);
            let Content::Text(content) = &entity.content else {
                panic!("text entity expected");
            };
            let Range::Text(range) = entity.source_range else {
                panic!("text range expected");
            };
            assert_eq!(
                crate::utils::slice_text(
                    &source.text,
                    range.start_byte_offset(),
                    range.end_byte_offset()
                )
                .expect("source range"),
                content
            );
            let mut covered = vec![false; content.len()];
            for fragment in &entity.fragments {
                let (start, end) = match fragment.range {
                    Range::Full => (0, content.len()),
                    Range::Byte(local) => (
                        usize::try_from(local.start_offset()).expect("fragment start"),
                        usize::try_from(local.end_offset()).expect("fragment end"),
                    ),
                    Range::Text(_) => panic!("fragments must store byte offsets"),
                };
                assert!(start < end && end <= content.len());
                assert!(content.is_char_boundary(start) && content.is_char_boundary(end));
                covered[start..end].fill(true);
            }
            assert!(
                content.char_indices().all(|(offset, character)| {
                    character.is_whitespace()
                        || covered[offset..offset + character.len_utf8()]
                            .iter()
                            .all(|value| *value)
                }),
                "every non-whitespace source byte belongs to a fragment"
            );
        }
    }

    #[test]
    fn falls_back_without_headings_and_validates_options() {
        let source = test_source(FileFormat::Markdown, "README.md", "plain markdown");
        let fragments = extract(&source, ChunkOptions::default()).expect("fallback");
        assert_eq!(fragments.len(), 1);
        assert_eq!(
            test_content(&fragments[0]),
            Content::Text("plain markdown".to_owned())
        );
        assert!(test_metadata(&fragments[0]).is_none());

        assert!(
            extract(
                &source,
                ChunkOptions {
                    max_chunk_chars: Some(0),
                    chunk_overlap_chars: None,
                }
            )
            .is_err()
        );
        assert!(
            extract(
                &source,
                ChunkOptions {
                    max_chunk_chars: Some(10),
                    chunk_overlap_chars: Some(10),
                }
            )
            .is_err()
        );
    }

    #[test]
    fn ignores_non_markdown_sources() {
        let source = test_source(FileFormat::Text, "README.txt", "# Heading");
        assert!(
            extract(&source, ChunkOptions::default())
                .expect("non-markdown")
                .is_empty()
        );
    }
}
