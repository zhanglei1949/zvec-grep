//! Tree-sitter stays in the host. Only owned, source-mapped facts cross crates.
mod bindings;
mod python;
mod rust;

use super::map_error;
use crate::{EngineError, EngineResult};
use tree_sitter::Node;
use zg_knowledge::{
    processing::ProcessingContext,
    relations::{
        Binding, BindingTarget, FactDiagnostic, FactNode, FileFacts, Language, Lookup, NodeKind,
        RelationKind, Scope, ScopeKind, Span, node_id,
    },
};

#[expect(
    clippy::too_many_lines,
    reason = "explicit language grammar dispatch kept together for conservative scope review"
)]
pub(crate) fn collect(
    root: Node<'_>,
    text: &str,
    facts: &mut FileFacts,
    context: &ProcessingContext,
) -> EngineResult<()> {
    if root.has_error() {
        facts.complete = false;
        facts.diagnostics.push(FactDiagnostic {
            code: "syntax_error".into(),
            span: Some(span(root)),
        });
    }
    facts.rules_fingerprint = zg_knowledge::processing::stable_digest(&(
        &facts.rules_fingerprint,
        "host-tree-sitter-rules-v4",
        match facts.language {
            Language::Rust => "tree-sitter-rust=0.24.2",
            _ => "tree-sitter-python=0.25.0",
        },
    ))
    .map_err(map_error)?;
    let mut work = vec![(root, 0usize, Vec::<String>::new())];
    while let Some((node, mut scope, mut qualified)) = work.pop() {
        context.check().map_err(map_error)?;
        let kind = node.kind();
        let definition = match (facts.language, kind) {
            (
                Language::Rust,
                "function_item" | "struct_item" | "enum_item" | "trait_item" | "type_item"
                | "const_item" | "static_item",
            )
            | (Language::Python, "function_definition" | "class_definition") => {
                Some(NodeKind::Symbol)
            }
            (Language::Rust, "mod_item") => Some(NodeKind::Module),
            _ => None,
        };
        if let Some(node_kind) = definition {
            if let Some(name_node) = node.child_by_field_name("name") {
                let name = content(name_node, text).to_owned();
                qualified.push(name.clone());
                let duplicate = facts
                    .nodes
                    .iter()
                    .filter(|n| n.qualified_name == qualified && n.kind == node_kind)
                    .count();
                let id = node_id(
                    &facts.version.path,
                    facts.language,
                    if node_kind == NodeKind::Module {
                        "module"
                    } else {
                        "symbol"
                    },
                    &qualified,
                    duplicate,
                )
                .map_err(map_error)?;
                // A decorated function can be replaced by an arbitrary callable.
                let decorated = node
                    .parent()
                    .is_some_and(|p| p.kind() == "decorated_definition");
                facts.nodes.push(FactNode {
                    id: id.clone(),
                    kind: node_kind,
                    entity_kind: entity_kind(node, facts.language),
                    name: name.clone(),
                    qualified_name: qualified.clone(),
                    parent: Some(facts.scopes[scope].owner.clone()),
                    path_addressable: (node_kind != NodeKind::Module
                        || node.child_by_field_name("body").is_some())
                        && !matches!(
                            facts.scopes[scope].kind,
                            ScopeKind::Function | ScopeKind::Block
                        ),
                    callable: matches!(kind, "function_item" | "function_definition")
                        && !decorated
                        && !(facts.language == Language::Rust
                            && matches!(
                                facts.scopes[scope].kind,
                                ScopeKind::Class | ScopeKind::UnsupportedImpl
                            )),
                    span: span(node),
                    entity_indices: Vec::new(),
                });
                facts.bindings.push(Binding {
                    scope,
                    name,
                    target: if decorated {
                        BindingTarget::Dynamic
                    } else {
                        BindingTarget::Node { id: id.clone() }
                    },
                    visible_from: 0,
                    visibility: rust::visibility(node, text),
                });
                if facts.language == Language::Rust && kind == "type_item" {
                    rust::type_alias(node, text, facts, scope, id.clone());
                }
                if matches!(
                    kind,
                    "function_item"
                        | "function_definition"
                        | "class_definition"
                        | "mod_item"
                        | "trait_item"
                ) {
                    let scope_kind = if matches!(kind, "class_definition" | "trait_item") {
                        ScopeKind::Class
                    } else if matches!(kind, "function_item" | "function_definition") {
                        ScopeKind::Function
                    } else {
                        ScopeKind::Module
                    };
                    scope = add_scope(facts, scope, id, scope_kind);
                    if facts.language == Language::Rust {
                        rust::generic_bindings(node, text, facts, scope);
                    }
                    if let Some(params) = node.child_by_field_name("parameters") {
                        parameters(params, text, facts, scope);
                    }
                }
            }
        } else if kind == "impl_item" {
            if let Some(t) = node.child_by_field_name("type") {
                qualified.push(format!("impl:{}", content(t, text)));
            }
            scope = rust::implementation(node, text, facts, scope);
        } else if facts.language == Language::Rust && kind == "block" {
            scope = add_scope(
                facts,
                scope,
                facts.scopes[scope].owner.clone(),
                ScopeKind::Block,
            );
        } else if matches!(kind, "lambda" | "closure_expression") {
            // Local callable parameters must not resolve to outer same-name defs.
            scope = add_scope(
                facts,
                scope,
                facts.scopes[scope].owner.clone(),
                if facts.language == Language::Rust {
                    ScopeKind::Closure
                } else {
                    ScopeKind::Function
                },
            );
            if let Some(params) = node.child_by_field_name("parameters") {
                parameters(params, text, facts, scope);
            }
        }
        match (facts.language, kind) {
            (Language::Python, "import_statement" | "import_from_statement") => {
                python_import(node, text, facts, scope)?;
            }
            (Language::Rust, "use_declaration") => {
                if let Some(arg) = node.child_by_field_name("argument") {
                    rust_import(arg, text, facts, scope, &[], &rust::visibility(node, text))?;
                }
            }
            (Language::Python, "assignment" | "augmented_assignment" | "named_expression") => {
                if let Some(left) = node
                    .child_by_field_name("left")
                    .or_else(|| node.child_by_field_name("name"))
                {
                    let target_scope = if kind == "named_expression" {
                        python::walrus_scope(facts, scope)
                    } else {
                        scope
                    };
                    bind_pattern(left, text, facts, target_scope, 0);
                }
            }
            (Language::Rust, "let_declaration") => {
                if let Some(p) = node.child_by_field_name("pattern") {
                    bind_pattern(p, text, facts, scope, node.end_byte());
                }
            }
            (Language::Python, "for_statement") => {
                if let Some(p) = node.child_by_field_name("left") {
                    bind_pattern(p, text, facts, scope, 0);
                }
            }
            (Language::Python, "delete_statement") => {
                bind_pattern(node, text, facts, scope, 0);
            }
            (Language::Python, "as_pattern" | "except_clause" | "except_group_clause") => {
                if let Some(alias) = node.child_by_field_name("alias") {
                    bind_pattern(alias, text, facts, scope, 0);
                }
            }
            (Language::Python, "case_pattern") => {
                python::case_bindings(node, text, facts, scope);
            }
            (Language::Python, "global_statement" | "nonlocal_statement") => {}
            (Language::Python, "type_alias_statement") => {
                bindings::incomplete(facts, "annotation_scope_unsupported", node);
            }
            (Language::Python, "type_parameter")
                if node.parent().is_some_and(|p| {
                    matches!(p.kind(), "function_definition" | "class_definition")
                }) =>
            {
                bindings::incomplete(facts, "annotation_scope_unsupported", node);
            }
            (Language::Rust, "call_expression") | (Language::Python, "call") => {
                if let Some(f) = node.child_by_field_name("function") {
                    let lookup = call_path(f, text, facts.language)
                        .map_or(Lookup::Dynamic, |parts| Lookup::Name { parts });
                    facts
                        .add_reference(scope, RelationKind::Call, span(f), lookup)
                        .map_err(map_error)?;
                }
            }
            _ => (),
        }
        facts.check_budget(context).map_err(map_error)?;
        let children = if facts.language == Language::Rust {
            rust::children(node, text, facts, scope)
        } else {
            python::children(node, text, facts, scope)
        };
        for (child, child_scope) in children.into_iter().rev() {
            work.push((child, child_scope, qualified.clone()));
        }
    }
    if facts.language == Language::Python {
        python::finish(facts);
    }
    Ok(())
}
fn add_scope(facts: &mut FileFacts, parent: usize, owner: String, kind: ScopeKind) -> usize {
    let index = facts.scopes.len();
    facts.scopes.push(Scope {
        parent: Some(parent),
        owner,
        kind,
    });
    index
}
fn span(n: Node<'_>) -> Span {
    Span {
        start: n.start_byte(),
        end: n.end_byte(),
    }
}
fn content<'a>(n: Node<'_>, text: &'a str) -> &'a str {
    text.get(n.start_byte()..n.end_byte()).unwrap_or("")
}
fn parameters(node: Node<'_>, text: &str, facts: &mut FileFacts, scope: usize) {
    let mut c = node.walk();
    for child in node.named_children(&mut c) {
        match child.kind() {
            "identifier"
            | "self"
            | "list_splat_pattern"
            | "dictionary_splat_pattern"
            | "tuple_pattern" => bind_pattern(child, text, facts, scope, 0),
            "parameter" => {
                if let Some(p) = child.child_by_field_name("pattern") {
                    bind_pattern(p, text, facts, scope, 0);
                }
            }
            "typed_parameter" => {
                if let Some(p) = child.named_child(0) {
                    bind_pattern(p, text, facts, scope, 0);
                }
            }
            "default_parameter" | "typed_default_parameter" => {
                if let Some(p) = child.child_by_field_name("name") {
                    bind_pattern(p, text, facts, scope, 0);
                }
            }
            _ => (),
        }
    }
}
fn bind_pattern(
    node: Node<'_>,
    text: &str,
    facts: &mut FileFacts,
    scope: usize,
    visible_from: usize,
) {
    bindings::bind(node, text, facts, scope, visible_from);
}
fn call_path(node: Node<'_>, text: &str, language: Language) -> Option<Vec<String>> {
    if language == Language::Rust {
        return rust::call_path(node, text);
    }
    let valid = match language {
        Language::Rust => matches!(node.kind(), "identifier" | "scoped_identifier"),
        Language::Python => matches!(node.kind(), "identifier" | "attribute"),
        Language::Markdown => false,
    };
    if !valid {
        return None;
    }
    if node.kind() == "attribute" {
        let object = node.child_by_field_name("object")?;
        let mut path = call_path(object, text, language)?;
        path.push(content(node.child_by_field_name("attribute")?, text).into());
        Some(path)
    } else {
        Some(content(node, text).split("::").map(str::to_owned).collect())
    }
}
fn add_import(
    facts: &mut FileFacts,
    scope: usize,
    name: String,
    path: Vec<String>,
    location: Span,
    visibility: &str,
) -> EngineResult<()> {
    facts.bindings.push(Binding {
        scope,
        name,
        target: BindingTarget::Import { path: path.clone() },
        visible_from: 0,
        visibility: visibility.into(),
    });
    facts
        .add_reference(
            scope,
            RelationKind::Import,
            location,
            Lookup::Import { path },
        )
        .map_err(map_error)
}
fn python_import(
    node: Node<'_>,
    text: &str,
    facts: &mut FileFacts,
    scope: usize,
) -> EngineResult<()> {
    let module = node.child_by_field_name("module_name");
    let mut c = node.walk();
    for child in node.named_children(&mut c) {
        if Some(child) == module {
            continue;
        }
        if child.kind() == "wildcard_import" {
            facts.complete = false;
            facts.diagnostics.push(FactDiagnostic {
                code: "wildcard_import_unsupported".into(),
                span: Some(span(child)),
            });
            continue;
        }
        if !matches!(child.kind(), "dotted_name" | "aliased_import") {
            continue;
        }
        let original = child.child_by_field_name("name").unwrap_or(child);
        let name = content(original, text);
        let alias = child.child_by_field_name("alias").map(|n| content(n, text));
        let mut path = Vec::new();
        if let Some(m) = module {
            let raw = content(m, text);
            let dots = raw.bytes().take_while(|b| *b == b'.').count();
            path.extend(std::iter::repeat_n(".".into(), dots));
            path.extend(
                raw[dots..]
                    .split('.')
                    .filter(|s| !s.is_empty())
                    .map(str::to_owned),
            );
        }
        path.extend(name.split('.').map(str::to_owned));
        let binding = alias
            .unwrap_or_else(|| name.split('.').next().unwrap_or(name))
            .to_owned();
        // `import a.b` binds `a`, whereas `import a.b as x` binds the full path.
        let binding_path = if module.is_none() && alias.is_none() {
            vec![binding.clone()]
        } else {
            path.clone()
        };
        facts
            .add_reference(
                scope,
                RelationKind::Import,
                span(child),
                Lookup::Import { path },
            )
            .map_err(map_error)?;
        facts.bindings.push(Binding {
            scope,
            name: binding,
            target: BindingTarget::Import { path: binding_path },
            visible_from: 0,
            visibility: String::new(),
        });
    }
    Ok(())
}
fn rust_import(
    node: Node<'_>,
    text: &str,
    facts: &mut FileFacts,
    scope: usize,
    prefix: &[String],
    visibility: &str,
) -> EngineResult<()> {
    if node.kind() == "scoped_use_list" {
        let mut path = prefix.to_vec();
        if let Some(p) = node.child_by_field_name("path") {
            path.extend(content(p, text).split("::").map(str::to_owned));
        }
        if let Some(list) = node.child_by_field_name("list") {
            rust_import(list, text, facts, scope, &path, visibility)?;
        }
    } else if node.kind() == "use_list" {
        let mut c = node.walk();
        for child in node.named_children(&mut c) {
            rust_import(child, text, facts, scope, prefix, visibility)?;
        }
    } else if node.kind() == "use_wildcard" {
        let mut path = prefix.to_vec();
        let raw = content(node, text)
            .trim_end_matches('*')
            .trim_end_matches("::");
        path.extend(raw.split("::").filter(|p| !p.is_empty()).map(str::to_owned));
        facts.bindings.push(Binding {
            scope,
            name: "*".into(),
            target: BindingTarget::GlobImport { path: path.clone() },
            visible_from: 0,
            visibility: visibility.into(),
        });
        facts
            .add_reference(
                scope,
                RelationKind::Import,
                span(node),
                Lookup::Import { path },
            )
            .map_err(map_error)?;
    } else {
        let mut path = prefix.to_vec();
        let p = if node.kind() == "use_as_clause" {
            node.child_by_field_name("path").unwrap_or(node)
        } else {
            node
        };
        let raw = content(p, text);
        if raw != "self" {
            path.extend(raw.split("::").map(str::to_owned));
        }
        let alias = node
            .child_by_field_name("alias")
            .map(|n| content(n, text).to_owned())
            .or_else(|| path.last().cloned())
            .ok_or_else(|| EngineError::invalid_argument("empty Rust import"))?;
        add_import(facts, scope, alias, path, span(node), visibility)?;
    }
    Ok(())
}

fn entity_kind(node: Node<'_>, language: Language) -> zg_knowledge::relations::EntityKind {
    use zg_knowledge::relations::EntityKind as K;
    match node.kind() {
        "mod_item" => K::Module,
        "class_definition" => K::Class,
        "trait_item" => K::Interface,
        "struct_item" | "enum_item" | "type_item" => K::Type,
        "const_item" | "static_item" => K::Constant,
        _ => {
            let mut parent = node.parent();
            while let Some(n) = parent {
                if matches!(n.kind(), "impl_item" | "trait_item" | "class_definition") {
                    return K::Method;
                }
                if matches!(
                    n.kind(),
                    "function_item" | "function_definition" | "mod_item" | "module" | "source_file"
                ) {
                    break;
                }
                parent = n.parent();
            }
            let _ = language;
            K::Function
        }
    }
}
