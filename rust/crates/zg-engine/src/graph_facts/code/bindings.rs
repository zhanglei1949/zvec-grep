use super::{Binding, BindingTarget, FactDiagnostic, FileFacts, Language, Node, content, span};

pub(super) fn incomplete(facts: &mut FileFacts, code: &str, node: Node<'_>) {
    facts.complete = false;
    facts.diagnostics.push(FactDiagnostic {
        code: code.into(),
        span: Some(span(node)),
    });
}

pub(super) fn dynamic(facts: &mut FileFacts, scope: usize, name: &str, visible_from: usize) {
    if name != "_" && !name.is_empty() {
        facts.bindings.push(Binding {
            scope,
            name: name.into(),
            target: BindingTarget::Dynamic,
            visible_from,
            visibility: String::new(),
        });
    }
}

/// Only pattern positions introduce bindings; constructor paths and field keys do not.
pub(super) fn bind(
    node: Node<'_>,
    text: &str,
    facts: &mut FileFacts,
    scope: usize,
    visible_from: usize,
) {
    let mut work = vec![node];
    while let Some(n) = work.pop() {
        match n.kind() {
            "identifier" | "self" | "shorthand_field_identifier" => {
                dynamic(facts, scope, content(n, text), visible_from);
            }
            "attribute"
            | "subscript"
            | "field_expression"
            | "scoped_identifier"
            | "scoped_type_identifier"
            | "type_identifier"
            | "generic_type"
            | "remaining_field_pattern" => {}
            "macro_invocation" if facts.language == Language::Rust => {
                incomplete(facts, "binding_macro_unsupported", n);
            }
            _ => {
                let skip = if facts.language == Language::Rust {
                    match n.kind() {
                        "struct_pattern" | "tuple_struct_pattern" => n.child_by_field_name("type"),
                        "field_pattern" if n.child_by_field_name("pattern").is_some() => {
                            n.child_by_field_name("name")
                        }
                        "match_pattern" => n.child_by_field_name("condition"),
                        _ => None,
                    }
                } else {
                    None
                };
                let mut c = n.walk();
                work.extend(
                    n.named_children(&mut c)
                        .filter(|child| Some(*child) != skip),
                );
            }
        }
    }
}
