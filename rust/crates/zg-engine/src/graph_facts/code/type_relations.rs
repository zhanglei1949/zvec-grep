//! Explicit type declarations only; runtime dispatch and generic selection stay unresolved.
use super::{FileFacts, Language, Lookup, Node, RelationKind, content, span};
use zg_knowledge::Result;

fn path(node: Node<'_>, text: &str) -> Option<Vec<String>> {
    match node.kind() {
        "identifier" | "type_identifier" | "crate" | "self" | "super" => {
            Some(vec![content(node, text).into()])
        }
        "scoped_type_identifier" | "scoped_identifier" => {
            let mut parts = path(node.child_by_field_name("path")?, text)?;
            parts.push(content(node.child_by_field_name("name")?, text).into());
            Some(parts)
        }
        "attribute" => {
            let mut parts = path(node.child_by_field_name("object")?, text)?;
            parts.push(content(node.child_by_field_name("attribute")?, text).into());
            Some(parts)
        }
        _ => None,
    }
}
fn emit(
    facts: &mut FileFacts,
    scope: usize,
    owner: &str,
    kind: RelationKind,
    node: Node<'_>,
    lookup: Lookup,
) -> Result<()> {
    facts.add_reference(scope, kind, span(node), lookup)?;
    facts
        .references
        .last_mut()
        .expect("reference just appended")
        .owner = owner.into();
    Ok(())
}
pub(super) fn declaration(
    node: Node<'_>,
    text: &str,
    facts: &mut FileFacts,
    scope: usize,
    owner: &str,
) -> Result<()> {
    let bounds = match (facts.language, node.kind()) {
        (Language::Rust, "trait_item") => node.child_by_field_name("bounds"),
        (Language::Python, "class_definition") => node.child_by_field_name("superclasses"),
        _ => return Ok(()),
    };
    let Some(bounds) = bounds else {
        return Ok(());
    };
    let mut cursor = bounds.walk();
    let unsupported = node.child_by_field_name("type_parameters").is_some()
        && facts.language == Language::Python
        || node
            .parent()
            .is_some_and(|p| p.kind() == "decorated_definition")
        || bounds
            .named_children(&mut cursor)
            .any(|n| n.kind() == "keyword_argument");
    let mut cursor = bounds.walk();
    for bound in bounds.named_children(&mut cursor) {
        if matches!(bound.kind(), "lifetime" | "keyword_argument") {
            continue;
        }
        let lookup = if unsupported {
            Lookup::Dynamic
        } else {
            path(bound, text).map_or(Lookup::Dynamic, |parts| Lookup::Name { parts })
        };
        emit(facts, scope, owner, RelationKind::Extends, bound, lookup)?;
    }
    Ok(())
}
pub(super) fn implementation(
    node: Node<'_>,
    text: &str,
    facts: &mut FileFacts,
    scope: usize,
) -> Result<()> {
    let Some(target) = node.child_by_field_name("trait") else {
        return Ok(());
    };
    let mut cursor = node.walk();
    let unsupported = node.child_by_field_name("type_parameters").is_some()
        || node
            .children(&mut cursor)
            .any(|n| n.kind() == "where_clause" || n.kind() == "!")
        || node
            .prev_named_sibling()
            .is_some_and(|n| n.kind() == "attribute_item");
    let lookup = if unsupported {
        Lookup::Dynamic
    } else {
        node.child_by_field_name("type")
            .and_then(|owner| path(owner, text))
            .zip(path(target, text))
            .map_or(Lookup::Dynamic, |(owner, target)| Lookup::TypeRelation {
                owner,
                target,
            })
    };
    facts.add_reference(scope, RelationKind::Implements, span(target), lookup)
}
