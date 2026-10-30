use super::bindings::{dynamic, incomplete};
use super::{
    Binding, BindingTarget, FactDiagnostic, FileFacts, Node, ScopeKind, add_scope, bind_pattern,
    content,
};

pub(super) fn walrus_scope(facts: &FileFacts, mut scope: usize) -> usize {
    while facts.scopes[scope].kind == ScopeKind::Comprehension {
        scope = facts.scopes[scope]
            .parent
            .expect("comprehension has enclosing scope");
    }
    scope
}

fn declarations(node: Node<'_>, text: &str, facts: &mut FileFacts, scope: usize) {
    let mut work = vec![node];
    while let Some(n) = work.pop() {
        if matches!(
            n.kind(),
            "function_definition" | "class_definition" | "lambda"
        ) {
            continue;
        }
        if matches!(n.kind(), "global_statement" | "nonlocal_statement") {
            let mut c = n.walk();
            for name in n
                .named_children(&mut c)
                .filter(|n| n.kind() == "identifier")
            {
                if scope == 0 {
                    if n.kind() == "nonlocal_statement" {
                        incomplete(facts, "invalid_nonlocal", n);
                    }
                    continue;
                }
                facts.bindings.push(Binding {
                    scope,
                    name: content(name, text).into(),
                    target: BindingTarget::Redirect {
                        scope: if n.kind() == "global_statement" {
                            0
                        } else {
                            usize::MAX
                        },
                    },
                    visible_from: 0,
                    visibility: String::new(),
                });
            }
        }
        let mut c = n.walk();
        work.extend(n.named_children(&mut c));
    }
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
        "function_definition" | "class_definition" | "lambda" => {
            let parent = facts.scopes[scope]
                .parent
                .expect("definition introduced scope");
            let body = node.child_by_field_name("body");
            if node.kind() != "lambda"
                && let Some(body) = body
            {
                declarations(body, text, facts, scope);
            }
            children
                .into_iter()
                .map(|n| (n, if Some(n) == body { scope } else { parent }))
                .collect()
        }
        "list_comprehension"
        | "dictionary_comprehension"
        | "set_comprehension"
        | "generator_expression" => {
            let inner = add_scope(
                facts,
                scope,
                facts.scopes[scope].owner.clone(),
                ScopeKind::Comprehension,
            );
            for clause in &children {
                if clause.kind() == "for_in_clause"
                    && let Some(p) = clause.child_by_field_name("left")
                {
                    bind_pattern(p, text, facts, inner, 0);
                }
            }
            let first = children
                .iter()
                .find(|n| n.kind() == "for_in_clause")
                .copied();
            let mut output = Vec::new();
            for child in children {
                if Some(child) == first {
                    let left = child.child_by_field_name("left");
                    let mut c = child.walk();
                    output.extend(
                        child
                            .named_children(&mut c)
                            .map(|n| (n, if Some(n) == left { inner } else { scope })),
                    );
                } else {
                    output.push((child, inner));
                }
            }
            output
        }
        _ => children.into_iter().map(|n| (n, scope)).collect(),
    }
}

pub(super) fn finish(facts: &mut FileFacts) {
    // Route declarations apply to the whole block, including bindings visited earlier.
    for i in 0..facts.bindings.len() {
        if matches!(
            facts.bindings[i].target,
            BindingTarget::Redirect { scope: usize::MAX }
        ) {
            let name = &facts.bindings[i].name;
            let mut parent = facts.scopes[facts.bindings[i].scope].parent;
            let mut target = None;
            while let Some(p) = parent {
                if matches!(
                    facts.scopes[p].kind,
                    ScopeKind::Function | ScopeKind::Comprehension
                ) && facts.bindings.iter().any(|b| {
                    b.scope == p
                        && &b.name == name
                        && !matches!(b.target, BindingTarget::Redirect { .. })
                }) {
                    target = Some(p);
                    break;
                }
                parent = facts.scopes[p].parent;
            }
            facts.bindings[i].target = target.map_or(BindingTarget::Dynamic, |scope| {
                BindingTarget::Redirect { scope }
            });
            if target.is_none() {
                facts.complete = false;
                facts.diagnostics.push(FactDiagnostic {
                    code: "invalid_nonlocal".into(),
                    span: None,
                });
            }
        }
    }
    let routes = facts
        .bindings
        .iter()
        .filter_map(|b| {
            if let BindingTarget::Redirect { scope } = b.target {
                Some((b.scope, b.name.clone(), scope))
            } else {
                None
            }
        })
        .collect::<Vec<_>>();
    for b in &mut facts.bindings {
        if !matches!(b.target, BindingTarget::Redirect { .. })
            && let Some((_, _, target)) = routes
                .iter()
                .find(|(scope, name, _)| *scope == b.scope && name == &b.name)
        {
            b.scope = *target;
        }
    }
}

pub(super) fn case_bindings(node: Node<'_>, text: &str, facts: &mut FileFacts, scope: usize) {
    let mut work = vec![node];
    while let Some(n) = work.pop() {
        let mut c = n.walk();
        let children = n.named_children(&mut c).collect::<Vec<_>>();
        match n.kind() {
            "dotted_name" => {
                if n.named_child_count() == 1
                    && let Some(name) = n.named_child(0)
                {
                    dynamic(facts, scope, content(name, text), 0);
                }
            }
            "class_pattern" | "keyword_pattern" => work.extend(children.into_iter().skip(1)),
            "dict_pattern" => {
                let mut c = n.walk();
                work.extend(n.children_by_field_name("value", &mut c));
                work.extend(children.into_iter().filter(|n| n.kind() == "splat_pattern"));
            }
            "identifier" => dynamic(facts, scope, content(n, text), 0),
            "string" | "concatenated_string" => {}
            _ => work.extend(children),
        }
    }
}
