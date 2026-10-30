use super::{
    Binding, BindingTarget, FileFacts, Node, ScopeKind, add_scope, bind_pattern, content, span,
};

pub(super) fn visibility(node: Node<'_>, text: &str) -> String {
    let mut c = node.walk();
    node.named_children(&mut c)
        .find(|n| n.kind() == "visibility_modifier")
        .map_or_else(String::new, |n| {
            content(n, text).split_whitespace().collect()
        })
}

pub(super) fn children<'a>(
    node: Node<'a>,
    text: &str,
    facts: &mut FileFacts,
    scope: usize,
) -> Vec<(Node<'a>, usize)> {
    let mut c = node.walk();
    let children = node.named_children(&mut c).collect::<Vec<_>>();
    match node.kind() {
        "if_expression" | "while_expression" => {
            let branch = add_scope(
                facts,
                scope,
                facts.scopes[scope].owner.clone(),
                ScopeKind::Block,
            );
            let condition = node.child_by_field_name("condition");
            let body = node.child_by_field_name(if node.kind() == "if_expression" {
                "consequence"
            } else {
                "body"
            });
            children
                .into_iter()
                .map(|n| {
                    (
                        n,
                        if Some(n) == condition || Some(n) == body {
                            branch
                        } else {
                            scope
                        },
                    )
                })
                .collect()
        }
        "let_condition" => {
            if let Some(p) = node.child_by_field_name("pattern") {
                bind_pattern(p, text, facts, scope, node.end_byte());
            }
            children.into_iter().map(|n| (n, scope)).collect()
        }
        "match_arm" => {
            let arm = add_scope(
                facts,
                scope,
                facts.scopes[scope].owner.clone(),
                ScopeKind::Block,
            );
            if let Some(p) = node.child_by_field_name("pattern") {
                bind_pattern(p, text, facts, arm, 0);
            }
            children.into_iter().map(|n| (n, arm)).collect()
        }
        "for_expression" => {
            let body_scope = add_scope(
                facts,
                scope,
                facts.scopes[scope].owner.clone(),
                ScopeKind::Block,
            );
            if let Some(p) = node.child_by_field_name("pattern") {
                bind_pattern(p, text, facts, body_scope, 0);
            }
            let body = node.child_by_field_name("body");
            children
                .into_iter()
                .map(|n| (n, if Some(n) == body { body_scope } else { scope }))
                .collect()
        }
        "enum_item" => {
            let owner = facts
                .nodes
                .iter()
                .find(|n| n.span == span(node))
                .expect("enum declared before its members")
                .id
                .clone();
            let members = add_scope(facts, scope, owner, ScopeKind::Enum);
            let body = node.child_by_field_name("body");
            if let Some(body) = body {
                let mut c = body.walk();
                for variant in body
                    .named_children(&mut c)
                    .filter(|n| n.kind() == "enum_variant")
                {
                    if let Some(name) = variant.child_by_field_name("name") {
                        facts.bindings.push(Binding {
                            scope: members,
                            name: content(name, text).into(),
                            target: BindingTarget::Dynamic,
                            visible_from: 0,
                            visibility: "pub".into(),
                        });
                    }
                }
            }
            children
                .into_iter()
                .map(|n| (n, if Some(n) == body { members } else { scope }))
                .collect()
        }
        _ => children.into_iter().map(|n| (n, scope)).collect(),
    }
}

// These scopes carry construction data only. Their owners remain lexical:
// an out-of-line impl must not become a child outside the type's source span.
pub(super) fn implementation(
    node: Node<'_>,
    text: &str,
    facts: &mut FileFacts,
    parent: usize,
) -> usize {
    let nominal = node.child_by_field_name("type");
    let path = nominal.and_then(|n| type_path(n, text, true));
    let supported = node.child_by_field_name("trait").is_none()
        && !has_child(node, "where_clause")
        && lifetime_only_impl(node, text)
        && facts.scopes[parent].kind == ScopeKind::Module;
    let kind = if node.child_by_field_name("trait").is_some() {
        ScopeKind::Class
    } else if supported {
        ScopeKind::InherentImpl
    } else {
        ScopeKind::UnsupportedImpl
    };
    if !supported {
        facts.diagnostics.push(super::FactDiagnostic {
            code: if kind == ScopeKind::Class {
                "trait_impl_resolution_unsupported"
            } else {
                "conditional_impl_resolution_unsupported"
            }
            .into(),
            span: Some(span(node)),
        });
    }
    let scope = add_scope(facts, parent, facts.scopes[parent].owner.clone(), kind);
    facts.bindings.push(Binding {
        scope,
        name: "Self".into(),
        target: path.map_or(BindingTarget::Dynamic, |path| BindingTarget::Import {
            path,
        }),
        visible_from: 0,
        visibility: String::new(),
    });
    generic_bindings(node, text, facts, scope);
    scope
}

pub(super) fn generic_bindings(node: Node<'_>, text: &str, facts: &mut FileFacts, scope: usize) {
    if let Some(params) = node.child_by_field_name("type_parameters") {
        let mut c = params.walk();
        for p in params.named_children(&mut c) {
            if let Some(name) = p.child_by_field_name("name") {
                super::bindings::dynamic(facts, scope, content(name, text), 0);
            }
        }
    }
}

pub(super) fn type_alias(
    node: Node<'_>,
    text: &str,
    facts: &mut FileFacts,
    parent: usize,
    owner: String,
) {
    let scope = add_scope(facts, parent, owner, ScopeKind::TypeAlias);
    let path = (node.child_by_field_name("type_parameters").is_none()
        && !has_child(node, "where_clause")
        && matches!(
            facts.scopes[parent].kind,
            ScopeKind::Module | ScopeKind::Block | ScopeKind::Function | ScopeKind::Closure
        ))
    .then(|| {
        node.child_by_field_name("type")
            .and_then(|n| type_path(n, text, false))
    })
    .flatten()
    .filter(|p| p.first().is_none_or(|name| name != "Self"));
    if path.is_none() {
        facts.diagnostics.push(super::FactDiagnostic {
            code: "type_alias_resolution_unsupported".into(),
            span: Some(span(node)),
        });
    }
    facts.bindings.push(Binding {
        scope,
        name: "Self".into(),
        target: path.map_or(BindingTarget::Dynamic, |path| BindingTarget::Import {
            path,
        }),
        visible_from: 0,
        visibility: String::new(),
    });
}
fn has_child(node: Node<'_>, kind: &str) -> bool {
    let mut c = node.walk();
    node.named_children(&mut c).any(|n| n.kind() == kind)
}
// Strip generic arguments only to identify an unsupported impl's nominal owner;
// they are never used to resolve a call or to infer constraints.
fn type_path(node: Node<'_>, text: &str, strip_arguments: bool) -> Option<Vec<String>> {
    match node.kind() {
        "type_identifier" | "identifier" | "self" | "super" | "crate" => {
            Some(vec![content(node, text).into()])
        }
        "scoped_type_identifier" | "scoped_identifier" => {
            let mut parts = type_path(node.child_by_field_name("path")?, text, strip_arguments)?;
            parts.push(content(node.child_by_field_name("name")?, text).into());
            Some(parts)
        }
        "generic_type" if strip_arguments => {
            type_path(node.child_by_field_name("type")?, text, true)
        }
        _ => None,
    }
}

pub(super) fn call_path(node: Node<'_>, text: &str) -> Option<Vec<String>> {
    if !matches!(node.kind(), "identifier" | "scoped_identifier") {
        return None;
    }
    type_path(node, text, false)
}

// Lifetimes may be ignored for nominal ownership only when they introduce no
// selection condition. Type/const arguments and concrete 'static impls retain
// UnsupportedImpl, so they can still prevent a false unique method target.
fn lifetime_only_impl(node: Node<'_>, text: &str) -> bool {
    let mut names = std::collections::BTreeSet::new();
    if let Some(parameters) = node.child_by_field_name("type_parameters") {
        let mut cursor = parameters.walk();
        for parameter in parameters.named_children(&mut cursor) {
            if parameter.kind() != "lifetime_parameter"
                || parameter.child_by_field_name("bounds").is_some()
            {
                return false;
            }
            let Some(name) = parameter.child_by_field_name("name") else {
                return false;
            };
            let name = content(name, text);
            if matches!(name, "'static" | "'_") || !names.insert(name) {
                return false;
            }
        }
    }
    node.child_by_field_name("type")
        .is_some_and(|n| lifetime_path(n, text, &names, &mut std::collections::BTreeSet::new()))
}
fn lifetime_path<'text>(
    node: Node<'_>,
    text: &'text str,
    names: &std::collections::BTreeSet<&str>,
    used: &mut std::collections::BTreeSet<&'text str>,
) -> bool {
    match node.kind() {
        "identifier" | "type_identifier" | "self" | "super" | "crate" => true,
        "scoped_identifier" | "scoped_type_identifier" => node
            .child_by_field_name("path")
            .is_some_and(|n| lifetime_path(n, text, names, used)),
        "generic_type" => {
            let Some(arguments) = node.child_by_field_name("type_arguments") else {
                return false;
            };
            let mut cursor = arguments.walk();
            arguments.named_child_count() > 0
                && arguments.named_children(&mut cursor).all(|argument| {
                    argument.kind() == "lifetime"
                        && (content(argument, text) == "'_"
                            || (names.contains(content(argument, text))
                                && used.insert(content(argument, text))))
                })
                && node
                    .child_by_field_name("type")
                    .is_some_and(|n| lifetime_path(n, text, names, used))
        }
        _ => false,
    }
}
