use super::super::{ScopeKind, add_scope};
use super::{
    BindingTarget, EngineResult, EntityKind, FileFacts, Language, Lookup, Node, RelationKind,
    children, content, diagnostic, dotted, dynamic, modifiers, reference, span, symbol,
};

pub(super) fn visit(
    node: Node<'_>,
    text: &str,
    facts: &mut FileFacts,
    mut scope: usize,
    mut blocked: bool,
) -> EngineResult<(usize, bool)> {
    if matches!(
        node.kind(),
        "preproc_if" | "preproc_ifdef" | "preproc_elif" | "preproc_else" | "template_declaration"
    ) {
        blocked = true;
        diagnostic(facts, node, "conditional_or_template_target_unresolved");
    }
    if children(node)
        .iter()
        .any(|child| child.kind() == "macro_annotation")
    {
        blocked = true;
        diagnostic(facts, node, "macro_declaration_target_unresolved");
    }
    match node.kind() {
        "namespace_definition" => {
            if let Some(name) = node.child_by_field_name("name") {
                let id = symbol(
                    facts,
                    scope,
                    node,
                    content(name, text),
                    EntityKind::Module,
                    false,
                )?;
                scope = add_scope(facts, scope, id, ScopeKind::Module);
            } else {
                blocked = true;
                diagnostic(facts, node, "anonymous_namespace_unresolved");
            }
        }
        "struct_specifier" | "class_specifier" | "union_specifier" | "enum_specifier" => {
            scope = type_declaration(node, text, facts, scope, blocked)?;
        }
        "function_definition" => scope = function(node, text, facts, scope, blocked)?,
        "declaration" | "field_declaration" => variables(node, text, facts, scope, blocked)?,
        "type_definition" => {
            if let Some(name) = node
                .child_by_field_name("declarator")
                .and_then(declarator_name)
            {
                let id = symbol(
                    facts,
                    scope,
                    node,
                    content(name, text),
                    EntityKind::Type,
                    false,
                )?;
                scope = add_scope(facts, scope, id, ScopeKind::Class);
            }
        }
        "parameter_declaration" | "optional_parameter_declaration" => {
            if let Some(declarator) = node
                .child_by_field_name("declarator")
                .and_then(declarator_name)
            {
                dynamic(facts, scope, declarator, text, 0);
            }
        }
        "function_declarator" if !definition_declarator(node) => {
            let owner = node
                .child_by_field_name("declarator")
                .and_then(declarator_name)
                .and_then(|name| {
                    facts
                        .nodes
                        .iter()
                        .find(|n| n.name == content(name, text) && n.span.contains(span(node)))
                })
                .map_or_else(|| facts.scopes[scope].owner.clone(), |n| n.id.clone());
            scope = add_scope(facts, scope, owner, ScopeKind::Function);
        }
        "compound_statement" | "for_statement" | "lambda_expression" => {
            scope = add_scope(
                facts,
                scope,
                facts.scopes[scope].owner.clone(),
                ScopeKind::Block,
            );
        }
        "preproc_def" | "preproc_function_def" => {
            if let Some(name) = node.child_by_field_name("name") {
                dynamic(facts, 0, name, text, 0);
            }
        }
        "preproc_include" => include_reference(node, text, facts, scope, blocked)?,
        "call_expression" => {
            if let Some(function) = node.child_by_field_name("function") {
                let lookup = if blocked {
                    Lookup::Dynamic
                } else {
                    dotted(function, text).map_or(Lookup::Dynamic, |parts| Lookup::Name { parts })
                };
                reference(facts, scope, RelationKind::Call, function, lookup)?;
            }
        }
        _ => (),
    }
    Ok((scope, blocked))
}
fn function_declarator(mut node: Node<'_>) -> Option<Node<'_>> {
    loop {
        match node.kind() {
            "function_declarator" => return Some(node),
            "pointer_declarator" | "reference_declarator" | "attributed_declarator" => {
                node = node.child_by_field_name("declarator")?;
            }
            _ => return None,
        }
    }
}
fn definition_declarator(mut node: Node<'_>) -> bool {
    while let Some(parent) = node.parent() {
        match parent.kind() {
            "function_definition" => return true,
            "pointer_declarator"
            | "reference_declarator"
            | "parenthesized_declarator"
            | "attributed_declarator" => node = parent,
            _ => return false,
        }
    }
    false
}
fn declarator_name(mut node: Node<'_>) -> Option<Node<'_>> {
    loop {
        match node.kind() {
            "identifier" | "field_identifier" | "operator_name" | "destructor_name" => {
                return Some(node);
            }
            "parenthesized_declarator" => node = node.named_child(0)?,
            "init_declarator"
            | "pointer_declarator"
            | "reference_declarator"
            | "function_declarator" => node = node.child_by_field_name("declarator")?,
            _ => return None,
        }
    }
}

fn access(node: Node<'_>, text: &str) -> String {
    let Some(parent) = node
        .parent()
        .filter(|p| p.kind() == "field_declaration_list")
    else {
        return String::new();
    };
    let mut sibling = node.prev_named_sibling();
    while let Some(previous) = sibling {
        if previous.kind() == "access_specifier" {
            return content(previous, text).into();
        }
        sibling = previous.prev_named_sibling();
    }
    if parent
        .parent()
        .is_some_and(|n| n.kind() == "class_specifier")
    {
        "private".into()
    } else {
        "public".into()
    }
}

fn type_declaration(
    node: Node<'_>,
    text: &str,
    facts: &mut FileFacts,
    mut scope: usize,
    blocked: bool,
) -> EngineResult<usize> {
    if node.child_by_field_name("body").is_none()
        && !node.parent().is_some_and(|p| {
            p.kind() == "declaration" && p.child_by_field_name("declarator").is_none()
        })
    {
        return Ok(scope);
    }
    if let Some(name) = node.child_by_field_name("name").filter(|n| {
        dotted(*n, text).is_some() || matches!(n.kind(), "operator_name" | "destructor_name")
    }) {
        let id = symbol(
            facts,
            scope,
            node,
            content(name, text),
            if matches!(node.kind(), "union_specifier" | "enum_specifier") {
                EntityKind::Type
            } else {
                EntityKind::Class
            },
            false,
        )?;
        facts
            .nodes
            .last_mut()
            .expect("type declaration")
            .path_addressable = node.child_by_field_name("body").is_some();
        facts.bindings.last_mut().expect("type binding").visibility = access(node, text);
        if blocked {
            facts.bindings.last_mut().expect("type binding").target = BindingTarget::Dynamic;
        }
        let old = scope;
        scope = add_scope(facts, scope, id.clone(), ScopeKind::Class);
        if facts.language == Language::Cpp {
            for clause in children(node)
                .into_iter()
                .filter(|n| n.kind() == "base_class_clause")
            {
                for target in children(clause)
                    .into_iter()
                    .filter(|n| !matches!(n.kind(), "access_specifier" | "attribute_declaration"))
                {
                    let lookup = if blocked {
                        Lookup::Dynamic
                    } else {
                        dotted(target, text).map_or(Lookup::Dynamic, |parts| Lookup::Name { parts })
                    };
                    reference(facts, old, RelationKind::Extends, target, lookup)?;
                    facts
                        .references
                        .last_mut()
                        .expect("base reference")
                        .owner
                        .clone_from(&id);
                }
            }
        }
    }
    Ok(scope)
}

fn function(
    node: Node<'_>,
    text: &str,
    facts: &mut FileFacts,
    mut scope: usize,
    blocked: bool,
) -> EngineResult<usize> {
    if let Some(declarator) = node.child_by_field_name("declarator")
        && let Some(function) = function_declarator(declarator)
        && let Some(name) = function.child_by_field_name("declarator").filter(|n| {
            dotted(*n, text).is_some() || matches!(n.kind(), "operator_name" | "destructor_name")
        })
    {
        let class = facts.scopes[scope].kind == ScopeKind::Class;
        let modifiers = modifiers(node, text);
        let callable = !blocked
            && !content(name, text).contains("::")
            && dotted(name, text).is_some()
            && (!class || modifiers.iter().any(|m| m == "static"))
            && !modifiers.iter().any(|m| m == "virtual");
        let id = symbol(
            facts,
            scope,
            node,
            content(name, text),
            if class {
                EntityKind::Method
            } else {
                EntityKind::Function
            },
            callable,
        )?;
        facts
            .bindings
            .last_mut()
            .expect("function binding")
            .visibility = format!("{} {}", modifiers.join(" "), access(node, text));
        if blocked {
            facts.bindings.last_mut().expect("function binding").target = BindingTarget::Dynamic;
        }
        scope = add_scope(facts, scope, id, ScopeKind::Function);
    }
    Ok(scope)
}

fn variables(
    node: Node<'_>,
    text: &str,
    facts: &mut FileFacts,
    scope: usize,
    blocked: bool,
) -> EngineResult<()> {
    for declaration in children(node).into_iter().filter(|n| {
        matches!(
            n.kind(),
            "identifier"
                | "field_identifier"
                | "init_declarator"
                | "pointer_declarator"
                | "function_declarator"
                | "reference_declarator"
        )
    }) {
        let is_function = function_declarator(declaration)
            .and_then(|f| f.child_by_field_name("declarator"))
            .is_some_and(|n| {
                matches!(
                    n.kind(),
                    "identifier" | "field_identifier" | "qualified_identifier"
                )
            });
        if let Some(name) = declarator_name(declaration) {
            if is_function || facts.scopes[scope].kind == ScopeKind::Class || scope == 0 {
                symbol(
                    facts,
                    scope,
                    node,
                    content(name, text),
                    if is_function {
                        if facts.scopes[scope].kind == ScopeKind::Class {
                            EntityKind::Method
                        } else {
                            EntityKind::Function
                        }
                    } else {
                        EntityKind::Field
                    },
                    false,
                )?;
                if blocked {
                    facts.bindings.last_mut().expect("declaration").target = BindingTarget::Dynamic;
                }
                if is_function {
                    facts.bindings.last_mut().expect("declaration").visibility =
                        "declaration".into();
                }
            } else {
                dynamic(facts, scope, name, text, declaration.end_byte());
            }
        }
    }
    Ok(())
}

fn include_reference(
    node: Node<'_>,
    text: &str,
    facts: &mut FileFacts,
    scope: usize,
    blocked: bool,
) -> EngineResult<()> {
    if let Some(path) = node.child_by_field_name("path") {
        let raw = content(path, text);
        let value = match path.kind() {
            "string_literal" => raw.strip_prefix('"').and_then(|s| s.strip_suffix('"')),
            "system_lib_string" => raw.strip_prefix('<').and_then(|s| s.strip_suffix('>')),
            _ => None,
        };
        reference(
            facts,
            scope,
            RelationKind::Import,
            path,
            if blocked {
                Lookup::Dynamic
            } else {
                value.map_or(Lookup::Dynamic, |path_value| Lookup::Include {
                    path: path_value.into(),
                    system: path.kind() == "system_lib_string",
                    target: None,
                })
            },
        )?;
    }
    Ok(())
}
