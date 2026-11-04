use super::super::{ScopeKind, add_scope};
use super::{
    Binding, BindingTarget, EngineResult, EntityKind, FileFacts, Lookup, Node, RelationKind,
    children, content, diagnostic, dotted, dynamic, modifiers, reference, symbol,
};

pub(super) fn module(root: Node<'_>, text: &str, facts: &mut FileFacts) {
    let package = children(root)
        .into_iter()
        .find(|n| n.kind() == "package_declaration")
        .and_then(|n| {
            children(n)
                .into_iter()
                .find(|c| matches!(c.kind(), "identifier" | "scoped_identifier"))
        })
        .and_then(|n| dotted(n, text))
        .unwrap_or_default();
    facts.module.as_mut().expect("code module").path = package;
}
pub(super) fn visit(
    node: Node<'_>,
    text: &str,
    facts: &mut FileFacts,
    mut scope: usize,
    blocked: bool,
) -> EngineResult<(usize, bool)> {
    match node.kind() {
        "class_declaration"
        | "interface_declaration"
        | "enum_declaration"
        | "record_declaration"
        | "annotation_type_declaration" => scope = type_declaration(node, text, facts, scope)?,
        "type_parameter" => {
            if let Some(name) = children(node)
                .into_iter()
                .find(|n| n.kind() == "type_identifier")
            {
                dynamic(facts, scope, name, text, 0);
            }
        }
        "method_declaration" | "constructor_declaration" => {
            scope = method(node, text, facts, scope)?;
        }
        "field_declaration" | "constant_declaration" => {
            for variable in children(node)
                .into_iter()
                .filter(|n| n.kind() == "variable_declarator")
            {
                if let Some(name) = variable.child_by_field_name("name") {
                    symbol(
                        facts,
                        scope,
                        node,
                        content(name, text),
                        EntityKind::Field,
                        false,
                    )?;
                }
            }
        }
        "formal_parameter" | "spread_parameter" | "catch_formal_parameter" => {
            if let Some(name) = node.child_by_field_name("name") {
                dynamic(facts, scope, name, text, 0);
            }
        }
        "variable_declarator"
            if !node.parent().is_some_and(|p| {
                matches!(p.kind(), "field_declaration" | "constant_declaration")
            }) =>
        {
            if let Some(name) = node.child_by_field_name("name") {
                dynamic(facts, scope, name, text, node.end_byte());
            }
        }
        "block" | "enhanced_for_statement" | "for_statement" => {
            scope = add_scope(
                facts,
                scope,
                facts.scopes[scope].owner.clone(),
                ScopeKind::Block,
            );
        }
        "lambda_expression" => {
            scope = add_scope(
                facts,
                scope,
                facts.scopes[scope].owner.clone(),
                ScopeKind::Block,
            );
            if let Some(parameters) = node.child_by_field_name("parameters") {
                if parameters.kind() == "identifier" {
                    dynamic(facts, scope, parameters, text, 0);
                } else {
                    for name in children(parameters)
                        .into_iter()
                        .filter(|n| n.kind() == "identifier")
                    {
                        dynamic(facts, scope, name, text, 0);
                    }
                }
            }
        }
        "import_declaration" => imports(node, text, facts, scope)?,
        "method_invocation" => {
            if let Some(name) = node.child_by_field_name("name") {
                let parts = match node.child_by_field_name("object") {
                    None => Some(vec![content(name, text).into()]),
                    Some(object) => dotted(object, text).map(|mut parts| {
                        parts.push(content(name, text).into());
                        parts
                    }),
                };
                reference(
                    facts,
                    scope,
                    RelationKind::Call,
                    node,
                    parts.map_or(Lookup::Dynamic, |parts| Lookup::Name { parts }),
                )?;
            }
        }
        _ => (),
    }
    Ok((scope, blocked))
}

fn type_declaration(
    node: Node<'_>,
    text: &str,
    facts: &mut FileFacts,
    mut scope: usize,
) -> EngineResult<usize> {
    if let Some(name) = node.child_by_field_name("name") {
        let id = symbol(
            facts,
            scope,
            node,
            content(name, text),
            if node.kind() == "interface_declaration" {
                EntityKind::Interface
            } else {
                EntityKind::Class
            },
            false,
        )?;
        facts.bindings.last_mut().expect("type binding").visibility =
            modifiers(node, text).join(" ");
        scope = add_scope(facts, scope, id.clone(), ScopeKind::Class);
        for clause in children(node).into_iter().filter(|n| {
            matches!(
                n.kind(),
                "superclass" | "super_interfaces" | "extends_interfaces"
            )
        }) {
            let kind = if clause.kind() == "super_interfaces" {
                RelationKind::Implements
            } else {
                RelationKind::Extends
            };
            let list = clause
                .named_child(0)
                .filter(|n| n.kind() == "type_list")
                .unwrap_or(clause);
            for target in children(list) {
                reference(
                    facts,
                    scope,
                    kind,
                    target,
                    dotted(target, text).map_or(Lookup::Dynamic, |parts| Lookup::Name { parts }),
                )?;
                facts
                    .references
                    .last_mut()
                    .expect("new reference")
                    .owner
                    .clone_from(&id);
            }
        }
    }
    Ok(scope)
}

fn method(
    node: Node<'_>,
    text: &str,
    facts: &mut FileFacts,
    mut scope: usize,
) -> EngineResult<usize> {
    if let Some(name) = node.child_by_field_name("name") {
        let modifiers = modifiers(node, text);
        let callable = node.kind() == "method_declaration"
            && modifiers.iter().any(|m| m == "static")
            && node.child_by_field_name("body").is_some();
        let id = symbol(
            facts,
            scope,
            node,
            content(name, text),
            EntityKind::Method,
            callable,
        )?;
        facts
            .bindings
            .last_mut()
            .expect("method binding")
            .visibility = modifiers.join(" ");
        scope = add_scope(facts, scope, id, ScopeKind::Function);
    }
    Ok(scope)
}

fn imports(node: Node<'_>, text: &str, facts: &mut FileFacts, scope: usize) -> EngineResult<()> {
    let wildcard = children(node).iter().any(|n| n.kind() == "asterisk");
    if let Some(name) = children(node)
        .into_iter()
        .find(|n| matches!(n.kind(), "identifier" | "scoped_identifier"))
        && let Some(path) = dotted(name, text)
    {
        reference(
            facts,
            scope,
            RelationKind::Import,
            name,
            Lookup::Import { path: path.clone() },
        )?;
        if wildcard {
            diagnostic(facts, node, "java_wildcard_import_unsupported");
        } else if let Some(name) = path.last() {
            facts.bindings.push(Binding {
                scope,
                name: name.clone(),
                target: BindingTarget::Import { path },
                visible_from: 0,
                visibility: String::new(),
            });
        }
    }
    Ok(())
}
