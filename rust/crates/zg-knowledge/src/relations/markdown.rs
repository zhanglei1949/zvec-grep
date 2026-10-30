use super::{
    Anchor, FactDiagnostic, FactNode, FileFacts, Language, Lookup, NodeKind, RelationKind,
    ResolutionStatus, ResolvedReference, SourceVersion, Span, node_id, resolved, target,
};
use crate::{Result, processing::ProcessingContext};

/// `CommonMark` offsets are used directly in the host's decoded UTF-8 buffer.
/// This parser never changes zg's retrieval segmentation or stores source text.
pub fn extract_markdown(
    version: SourceVersion,
    text: &str,
    context: &ProcessingContext,
) -> Result<FileFacts> {
    context.check_input(text.len())?;
    let mut facts = FileFacts::new(version, Language::Markdown, None, text)?;
    // Locked parser and anchor policy are part of the graph-only rebuild key.
    facts.rules_fingerprint = crate::processing::stable_digest(&(
        &facts.rules_fingerprint,
        zg_markdown::PARSER_VERSION,
        "explicit-md-links+unicode-slug-v1",
    ))?;
    let mut headings: Vec<zg_markdown::Heading> = Vec::new();
    let mut links: Vec<zg_markdown::Link> = Vec::new();
    for node in zg_markdown::parse(text) {
        context.check()?;
        match node {
            zg_markdown::MdNode::Heading(heading) => headings.push(heading),
            zg_markdown::MdNode::Link(link) => links.push(link),
        }
        context.check_output(2 + headings.len() + links.len())?;
    }
    append_sections(&mut facts, &headings, text.len(), context)?;
    for link in links {
        let span = Span {
            start: link.span.start,
            end: link.span.end,
        };
        facts.add_reference(
            0,
            RelationKind::DocumentLink,
            span,
            Lookup::Document {
                destination: link.destination,
            },
        )?;
        let owner = facts
            .nodes
            .iter()
            .rev()
            .find(|n| n.kind == NodeKind::Section && n.span.contains(span))
            .unwrap_or(&facts.nodes[0])
            .id
            .clone();
        if let Some(reference) = facts.references.last_mut() {
            reference.owner = owner;
        }
        facts.check_budget(context)?;
    }
    facts.validate(text, context)?;
    Ok(facts)
}

fn append_sections(
    facts: &mut FileFacts,
    headings: &[zg_markdown::Heading],
    text_len: usize,
    context: &ProcessingContext,
) -> Result<()> {
    let mut parents: Vec<(u8, String)> = Vec::new();
    for (index, heading) in headings.iter().enumerate() {
        let level = heading.level;
        let end = headings[index + 1..]
            .iter()
            .find(|next| next.level <= level)
            .map_or(text_len, |next| next.span.start);
        let slug = heading.slug.clone();
        let discriminator = heading.anchor_discriminator;
        if discriminator > 0 {
            facts.diagnostics.push(FactDiagnostic {
                code: "duplicate_heading_anchor".into(),
                span: Some(Span {
                    start: heading.span.start,
                    end,
                }),
            });
        }
        let id = node_id(
            &facts.version.path,
            Language::Markdown,
            "section",
            std::slice::from_ref(&slug),
            discriminator,
        )?;
        while parents.last().is_some_and(|(l, _)| *l >= level) {
            parents.pop();
        }
        let parent = parents
            .last()
            .map_or_else(|| facts.nodes[0].id.clone(), |(_, id)| id.clone());
        facts.nodes.push(FactNode {
            id: id.clone(),
            kind: NodeKind::Section,
            entity_kind: super::EntityKind::Section,
            name: heading.text.clone(),
            qualified_name: vec![slug.clone()],
            parent: Some(parent),
            path_addressable: false,
            callable: false,
            span: Span {
                start: heading.span.start,
                end,
            },
            entity_indices: Vec::new(),
        });
        facts.anchors.push(Anchor {
            name: slug,
            node_id: id.clone(),
        });
        parents.push((level, id));
        facts.check_budget(context)?;
    }
    Ok(())
}

/// The host supplies the complete selected-file inventory, including indexed
/// code targets. Only Markdown facts are ever connected. No disk reads occur.
pub fn resolve_documents(
    files: &[FileFacts],
    indexed_paths: &[String],
    context: &ProcessingContext,
) -> Result<Vec<ResolvedReference>> {
    let mut output = Vec::new();
    visit_documents(files, indexed_paths, context, |r| {
        output.push(r);
        Ok(())
    })?;
    Ok(output)
}

pub(crate) fn visit_documents(
    files: &[FileFacts],
    indexed_paths: &[String],
    context: &ProcessingContext,
    mut emit: impl FnMut(ResolvedReference) -> Result<()>,
) -> Result<()> {
    for f in files {
        f.validate_structure(context)?;
    }
    for path in indexed_paths {
        super::validate_path(path)?;
    }
    let documents: Vec<_> = files
        .iter()
        .filter(|f| f.language == Language::Markdown)
        .collect();
    let dependencies = documents
        .iter()
        .map(|f| f.version.clone())
        .collect::<Vec<_>>();
    let mut emitted = 0usize;
    let mut output_cost = 0usize;
    for file in &documents {
        for reference in &file.references {
            context.check()?;
            let Lookup::Document { destination } = &reference.lookup else {
                continue;
            };
            if !file.complete {
                output_cost = output_cost.saturating_add(1 + dependencies.len());
                context.check_output(output_cost)?;
                emit(resolved(
                    file,
                    reference,
                    Vec::new(),
                    ResolutionStatus::Unresolved,
                    "incomplete_parse",
                    dependencies.clone(),
                ))?;
                continue;
            }
            let (path, anchor) = match link_target(&file.version.path, destination) {
                Ok(target) => target,
                Err((status, reason)) => {
                    output_cost = output_cost.saturating_add(1 + dependencies.len());
                    context.check_output(output_cost)?;
                    emit(resolved(
                        file,
                        reference,
                        Vec::new(),
                        status,
                        reason,
                        dependencies.clone(),
                    ))?;
                    emitted += 1;
                    context.check_output(emitted)?;
                    continue;
                }
            };
            let target_exists = documents
                .iter()
                .any(|f| f.version.path == path && f.complete);
            let is_indexed = indexed_paths.contains(&path);
            let is_markdown = path
                .rsplit('.')
                .next()
                .is_some_and(|s| matches!(s.to_ascii_lowercase().as_str(), "md" | "markdown"));
            let (missing, reason) = if !is_markdown && is_indexed {
                (ResolutionStatus::Unsupported, "unsupported_graph_target")
            } else if !is_markdown {
                (ResolutionStatus::Unresolved, "target_not_indexed")
            } else if !target_exists {
                (ResolutionStatus::Unresolved, "document_not_indexed")
            } else {
                (ResolutionStatus::Unresolved, "anchor_not_found")
            };
            let candidates = if is_markdown && is_indexed {
                document_candidates(&documents, &path, &anchor)
            } else {
                Vec::new()
            };
            let reason = if candidates.is_empty() {
                reason
            } else {
                "explicit_document_link"
            };
            output_cost = output_cost.saturating_add(1 + dependencies.len() + candidates.len());
            context.check_output(output_cost)?;
            emit(resolved(
                file,
                reference,
                candidates,
                missing,
                reason,
                dependencies.clone(),
            ))?;
            emitted += 1;
            context.check_output(emitted)?;
        }
    }
    Ok(())
}

fn document_candidates(files: &[&FileFacts], path: &str, anchor: &str) -> Vec<super::Target> {
    let mut candidates = Vec::new();
    for f in files
        .iter()
        .filter(|f| f.version.path == path && f.complete)
    {
        if anchor.is_empty() {
            candidates.push(target(f, &f.nodes[0]));
        } else {
            for a in f.anchors.iter().filter(|a| a.name == anchor) {
                if let Some(n) = f.nodes.iter().find(|n| n.id == a.node_id) {
                    candidates.push(target(f, n));
                }
            }
        }
    }
    candidates
}

type LinkError = (ResolutionStatus, &'static str);
fn link_target(
    source: &str,
    destination: &str,
) -> std::result::Result<(String, String), LinkError> {
    if destination.starts_with("//")
        || destination
            .split(['/', '#', '?'])
            .next()
            .is_some_and(|s| s.contains(':'))
    {
        return Err((ResolutionStatus::External, "external_link"));
    }
    if destination.starts_with('/') || destination.contains('\\') {
        return Err((ResolutionStatus::OutsideWorkspace, "outside_workspace"));
    }
    let (path, anchor) = destination.split_once('#').unwrap_or((destination, ""));
    if path.contains('?') {
        return Err((ResolutionStatus::Unsupported, "query_link_unsupported"));
    }
    let path = percent_decode(path)?;
    let anchor = percent_decode(anchor)?;
    // Reject decoded absolute/scheme/backslash escapes as well.
    if path.starts_with('/') || path.contains(['\\', '\0', ':']) || anchor.contains('\0') {
        return Err((ResolutionStatus::OutsideWorkspace, "outside_workspace"));
    }
    if path.is_empty() {
        return Ok((source.into(), anchor));
    }
    let mut components: Vec<_> = source.split('/').collect();
    components.pop();
    for component in path.split('/') {
        match component {
            "" | "." => (),
            ".." => {
                if components.pop().is_none() {
                    return Err((ResolutionStatus::OutsideWorkspace, "outside_workspace"));
                }
            }
            part => components.push(part),
        }
    }
    Ok((components.join("/"), anchor))
}
fn percent_decode(value: &str) -> std::result::Result<String, LinkError> {
    let bytes = value.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let code = bytes
                .get(i + 1..i + 3)
                .and_then(|s| std::str::from_utf8(s).ok())
                .and_then(|s| u8::from_str_radix(s, 16).ok())
                .ok_or((ResolutionStatus::Unresolved, "invalid_link_encoding"))?;
            out.push(code);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).map_err(|_| (ResolutionStatus::Unresolved, "invalid_link_encoding"))
}
