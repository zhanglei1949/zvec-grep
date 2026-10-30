//! Shared `CommonMark` structural parsing for zg.
//!
//! Produces a neutral heading/link stream with decoded UTF-8 byte offsets that
//! both the engine's retrieval segmenter and the knowledge graph's fact builder
//! consume. This crate owns no retrieval, graph or storage concept: section
//! spanning, chunking, node identity and link resolution stay with consumers.
//! The parse is lazy so a consumer can enforce its own budget and cancellation
//! per item and stop early without materializing a whole document.

use std::{collections::HashMap, ops::Range};

use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};

/// Parser identity folded into consumer rule fingerprints. Pin the exact
/// `pulldown-cmark` version and options here; changing either must change this
/// string so downstream rebuild keys notice.
pub const PARSER_VERSION: &str = "pulldown-cmark=0.13.5";

/// One `ATX` or setext heading with its `GitHub`-style anchor slug.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Heading {
    /// `CommonMark` heading level, 1..=6.
    pub level: u8,
    /// Inline content with markup flattened; code spans contribute their text.
    pub text: String,
    /// `GitHub`-style anchor slug derived from `text`.
    pub slug: String,
    /// Decoded UTF-8 byte span of the heading markup itself.
    pub span: Range<usize>,
    /// Count of earlier headings in the same document that share `slug`.
    pub anchor_discriminator: usize,
}

/// One link, including autolinks. Classification and resolution are the
/// consumer's job; `destination` is the raw target text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Link {
    /// Decoded UTF-8 byte span of the link markup.
    pub span: Range<usize>,
    /// Raw destination before any resolution or percent-decoding.
    pub destination: String,
}

/// A structural item yielded in document order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MdNode {
    /// A heading completed at its closing event.
    Heading(Heading),
    /// A link at its opening event.
    Link(Link),
}

/// `GitHub`-style anchor slug: lowercase, whitespace to `-`, keep alphanumerics
/// and `-`/`_`, drop everything else.
#[must_use]
pub fn anchor_slug(title: &str) -> String {
    title
        .to_lowercase()
        .chars()
        .filter_map(|c| {
            if c.is_whitespace() {
                Some('-')
            } else if c.is_alphanumeric() || matches!(c, '-' | '_') {
                Some(c)
            } else {
                None
            }
        })
        .collect()
}

/// Lazily parses `CommonMark` (with tables enabled) into headings and links in
/// document order. Fenced and indented code is skipped by the parser, so
/// heading-like lines inside code never surface.
pub fn parse(text: &str) -> impl Iterator<Item = MdNode> + '_ {
    Parse {
        inner: Parser::new_ext(text, Options::ENABLE_TABLES).into_offset_iter(),
        pending: None,
        slug_counts: HashMap::new(),
    }
}

struct Parse<'a> {
    inner: pulldown_cmark::OffsetIter<'a>,
    pending: Option<(usize, u8, String)>,
    slug_counts: HashMap<String, usize>,
}

impl Iterator for Parse<'_> {
    type Item = MdNode;

    fn next(&mut self) -> Option<MdNode> {
        loop {
            let (event, range) = self.inner.next()?;
            match event {
                Event::Start(Tag::Heading { level, .. }) => {
                    self.pending = Some((range.start, level as u8, String::new()));
                }
                Event::End(TagEnd::Heading(_)) => {
                    if let Some((start, level, text)) = self.pending.take() {
                        let slug = anchor_slug(&text);
                        let seen = self.slug_counts.entry(slug.clone()).or_default();
                        let anchor_discriminator = *seen;
                        *seen += 1;
                        return Some(MdNode::Heading(Heading {
                            level,
                            text,
                            slug,
                            span: start..range.end,
                            anchor_discriminator,
                        }));
                    }
                }
                Event::Text(value) | Event::Code(value) => {
                    if let Some((_, _, title)) = &mut self.pending {
                        title.push_str(&value);
                    }
                }
                Event::SoftBreak | Event::HardBreak => {
                    if let Some((_, _, title)) = &mut self.pending {
                        title.push(' ');
                    }
                }
                Event::Start(Tag::Link { dest_url, .. }) => {
                    return Some(MdNode::Link(Link {
                        span: range.start..range.end,
                        destination: dest_url.into_string(),
                    }));
                }
                _ => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{MdNode, anchor_slug, parse};

    type HeadingRow = (u8, String, String, usize);

    fn collected(text: &str) -> (Vec<HeadingRow>, Vec<String>) {
        let mut headings = Vec::new();
        let mut links = Vec::new();
        for node in parse(text) {
            match node {
                MdNode::Heading(h) => {
                    headings.push((h.level, h.text, h.slug, h.anchor_discriminator));
                }
                MdNode::Link(l) => links.push(l.destination),
            }
        }
        (headings, links)
    }

    #[test]
    fn detects_atx_and_setext_with_slugs_and_discriminators() {
        let (headings, _) = collected("# Hello *World*\n## Same\n## Same\nNext\n===\n");
        assert_eq!(
            headings,
            vec![
                (1, "Hello World".into(), "hello-world".into(), 0),
                (2, "Same".into(), "same".into(), 0),
                (2, "Same".into(), "same".into(), 1),
                (1, "Next".into(), "next".into(), 0),
            ]
        );
    }

    #[test]
    fn skips_fenced_code_and_collects_only_real_links() {
        let (headings, links) = collected("# T\n```md\n# fake\n[bad](x.md)\n```\n[ok](y.md#sec)\n");
        assert_eq!(headings, vec![(1, "T".into(), "t".into(), 0)]);
        assert_eq!(links, vec!["y.md#sec".to_string()]);
    }

    #[test]
    fn heading_spans_are_ordered_and_within_the_source() {
        let text = "# One\ntext\n## Two\n";
        let mut previous_end = 0;
        for node in parse(text) {
            if let MdNode::Heading(h) = node {
                assert!(h.span.start >= previous_end, "headings keep document order");
                assert!(h.span.start < h.span.end, "span is non-empty");
                assert!(h.span.end <= text.len(), "span stays within the source");
                assert!(
                    text[h.span.start..h.span.end].starts_with('#'),
                    "span covers the ATX marker"
                );
                previous_end = h.span.end;
            }
        }
    }

    #[test]
    fn slug_matches_the_github_style_contract() {
        assert_eq!(anchor_slug("Hello *World*"), "hello-world");
        assert_eq!(anchor_slug("中文标题"), "中文标题");
        assert_eq!(anchor_slug("Emoji 🎉 Heading"), "emoji--heading");
        assert_eq!(anchor_slug("A_B-C"), "a_b-c");
    }
}
