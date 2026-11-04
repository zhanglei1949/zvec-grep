//! The additional grammars share the existing owned facts and lexical scopes.
mod c_family;
mod go;
mod java;
use super::{
    Binding, BindingTarget, EngineResult, FactDiagnostic, FactNode, FileFacts, Language, Lookup,
    Node, NodeKind, ProcessingContext, RelationKind, content, map_error, node_id, span,
};
use zg_knowledge::relations::EntityKind;

pub(super) fn collect(
    root: Node<'_>,
    text: &str,
    facts: &mut FileFacts,
    context: &ProcessingContext,
) -> EngineResult<()> {
    match facts.language {
        Language::Go => go::module(root, text, facts),
        Language::Java => java::module(root, text, facts),
        _ => (),
    }
    let mut work = vec![(root, 0, false)];
    while let Some((node, scope, blocked)) = work.pop() {
        context.check().map_err(map_error)?;
        let (scope, blocked) = match facts.language {
            Language::Go => go::visit(node, text, facts, scope, blocked)?,
            Language::Java => java::visit(node, text, facts, scope, blocked)?,
            Language::C | Language::Cpp => c_family::visit(node, text, facts, scope, blocked)?,
            _ => unreachable!("compiled language dispatch"),
        };
        facts.check_budget(context).map_err(map_error)?;
        let mut cursor = node.walk();
        for child in node
            .named_children(&mut cursor)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
        {
            work.push((child, scope, blocked));
        }
    }
    facts.rules_fingerprint = super::compiled_fingerprint(facts, None)?;
    Ok(())
}
fn children(node: Node<'_>) -> Vec<Node<'_>> {
    let mut cursor = node.walk();
    node.named_children(&mut cursor).collect()
}
fn symbol(
    facts: &mut FileFacts,
    scope: usize,
    node: Node<'_>,
    name: &str,
    kind: EntityKind,
    callable: bool,
) -> EngineResult<String> {
    let owner = &facts.scopes[scope].owner;
    let mut qualified = facts
        .nodes
        .iter()
        .find(|n| &n.id == owner)
        .expect("scope owner")
        .qualified_name
        .clone();
    let parts = name.split("::").map(str::to_owned).collect::<Vec<_>>();
    qualified.extend(parts);
    let name = qualified.last().expect("named declaration").clone();
    let duplicate = facts
        .nodes
        .iter()
        .filter(|n| n.qualified_name == qualified)
        .count();
    let id = node_id(
        &facts.version.path,
        facts.language,
        "symbol",
        &qualified,
        duplicate,
    )
    .map_err(map_error)?;
    facts.nodes.push(FactNode {
        id: id.clone(),
        kind: if kind == EntityKind::Module {
            NodeKind::Module
        } else {
            NodeKind::Symbol
        },
        entity_kind: kind,
        name: name.clone(),
        qualified_name: qualified,
        parent: Some(owner.clone()),
        path_addressable: true,
        callable,
        span: span(node),
        entity_indices: vec![],
    });
    facts.bindings.push(Binding {
        scope,
        name,
        target: BindingTarget::Node { id: id.clone() },
        visible_from: 0,
        visibility: String::new(),
    });
    Ok(id)
}
fn dynamic(facts: &mut FileFacts, scope: usize, node: Node<'_>, text: &str, visible_from: usize) {
    facts.bindings.push(Binding {
        scope,
        name: content(node, text).into(),
        target: BindingTarget::Dynamic,
        visible_from,
        visibility: String::new(),
    });
}
fn reference(
    facts: &mut FileFacts,
    scope: usize,
    kind: RelationKind,
    node: Node<'_>,
    lookup: Lookup,
) -> EngineResult<()> {
    facts
        .add_reference(scope, kind, span(node), lookup)
        .map_err(map_error)
}
fn diagnostic(facts: &mut FileFacts, node: Node<'_>, code: &str) {
    facts.diagnostics.push(FactDiagnostic {
        code: code.into(),
        span: Some(span(node)),
    });
}
fn dotted(node: Node<'_>, text: &str) -> Option<Vec<String>> {
    match node.kind() {
        "identifier"
        | "type_identifier"
        | "field_identifier"
        | "namespace_identifier"
        | "package_identifier" => Some(vec![content(node, text).into()]),
        "scoped_identifier" | "scoped_type_identifier" | "qualified_identifier" => {
            let value = content(node, text);
            let mut parts = value
                .split(['.', ':'])
                .filter(|p| !p.is_empty())
                .map(str::to_owned)
                .collect::<Vec<_>>();
            if parts.is_empty()
                || !parts
                    .iter()
                    .all(|p| p.chars().all(|c| c.is_alphanumeric() || c == '_'))
            {
                return None;
            }
            if value.starts_with("::") {
                // Keep root qualification in the existing name-path representation.
                parts.insert(0, "::".into());
            }
            Some(parts)
        }
        _ => None,
    }
}
fn modifiers(node: Node<'_>, text: &str) -> Vec<String> {
    children(node)
        .into_iter()
        .filter(|c| {
            matches!(
                c.kind(),
                "modifiers" | "storage_class_specifier" | "virtual_specifier"
            )
        })
        .flat_map(|c| {
            content(c, text)
                .split_whitespace()
                .map(str::to_owned)
                .collect::<Vec<_>>()
        })
        .collect()
}
