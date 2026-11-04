use super::super::{ScopeKind, add_scope};
use super::{
    Binding, BindingTarget, EngineResult, EntityKind, FileFacts, Lookup, Node, RelationKind,
    children, content, diagnostic, dotted, dynamic, reference, symbol,
};

pub(super) fn module(root: Node<'_>, text: &str, facts: &mut FileFacts) {
    if text.lines().any(|line| {
        line.trim_start().starts_with("//go:build") || line.trim_start().starts_with("// +build")
    }) {
        facts.complete = false;
        diagnostic(facts, root, "go_build_constraints_unselected");
    }
    let path = std::path::Path::new(&facts.version.path);
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
    let Some((_, suffix)) = stem.rsplit_once('_') else {
        return;
    };
    // Go 1.24 internal/syslist: filename constraints include retired targets.
    if [
        "test",
        "aix",
        "android",
        "darwin",
        "dragonfly",
        "freebsd",
        "hurd",
        "illumos",
        "ios",
        "js",
        "linux",
        "nacl",
        "netbsd",
        "openbsd",
        "plan9",
        "solaris",
        "wasip1",
        "windows",
        "zos",
        "386",
        "amd64",
        "amd64p32",
        "arm",
        "armbe",
        "arm64",
        "arm64be",
        "loong64",
        "mips",
        "mipsle",
        "mips64",
        "mips64le",
        "mips64p32",
        "mips64p32le",
        "ppc",
        "ppc64",
        "ppc64le",
        "riscv",
        "riscv64",
        "s390",
        "s390x",
        "sparc",
        "sparc64",
        "wasm",
    ]
    .contains(&suffix)
    {
        facts.complete = false;
        diagnostic(facts, root, "go_build_context_unselected");
    }
}
pub(super) fn visit(
    node: Node<'_>,
    text: &str,
    facts: &mut FileFacts,
    mut scope: usize,
    blocked: bool,
) -> EngineResult<(usize, bool)> {
    match node.kind() {
        "package_clause" => {
            if let Some(name) = node.named_child(0) {
                symbol(
                    facts,
                    scope,
                    node,
                    content(name, text),
                    EntityKind::Module,
                    false,
                )?;
                facts.bindings.pop(); // A package clause does not bind a local identifier.
            }
        }
        "function_declaration" | "method_declaration" | "method_elem" | "method_spec" => {
            scope = function(node, text, facts, scope)?;
        }
        "type_spec" | "type_alias" => {
            if let Some(name) = node.child_by_field_name("name") {
                let kind = match node.child_by_field_name("type").map(|n| n.kind()) {
                    Some("struct_type") => EntityKind::Class,
                    Some("interface_type") => EntityKind::Interface,
                    _ => EntityKind::Type,
                };
                let id = symbol(facts, scope, node, content(name, text), kind, false)?;
                scope = add_scope(facts, scope, id, ScopeKind::Class);
            }
        }
        "field_declaration" | "type_elem" => fields(node, text, facts, scope)?,
        "import_spec" => imports(node, text, facts, scope)?,
        "block"
        | "for_statement"
        | "if_statement"
        | "expression_switch_statement"
        | "type_switch_statement" => {
            scope = add_scope(
                facts,
                scope,
                facts.scopes[scope].owner.clone(),
                ScopeKind::Block,
            );
        }
        "func_literal" => {
            scope = add_scope(
                facts,
                scope,
                facts.scopes[scope].owner.clone(),
                ScopeKind::Function,
            );
        }
        "parameter_declaration" | "variadic_parameter_declaration" => {
            for name in children(node)
                .into_iter()
                .filter(|n| n.kind() == "identifier")
            {
                dynamic(facts, scope, name, text, 0);
            }
        }
        "short_var_declaration" | "range_clause" => {
            let mut cursor = node.walk();
            if node.children(&mut cursor).any(|child| child.kind() == ":=")
                && let Some(left) = node.child_by_field_name("left")
            {
                for name in children(left)
                    .into_iter()
                    .filter(|n| n.kind() == "identifier")
                {
                    dynamic(facts, scope, name, text, node.end_byte());
                }
            }
        }
        "var_spec" | "const_spec" => {
            for name in children(node)
                .into_iter()
                .filter(|n| n.kind() == "identifier")
            {
                dynamic(
                    facts,
                    scope,
                    name,
                    text,
                    if scope == 0 { 0 } else { node.end_byte() },
                );
            }
        }
        "call_expression" => call(node, text, facts, scope)?,
        _ => (),
    }
    Ok((scope, blocked))
}
fn receiver_type(node: Node<'_>, text: &str) -> Option<String> {
    let parameter = node.named_child(0)?;
    let mut kind = parameter.child_by_field_name("type")?;
    if kind.kind() == "pointer_type" {
        kind = kind.named_child(0)?;
    }
    (kind.kind() == "type_identifier").then(|| content(kind, text).into())
}

fn imports(node: Node<'_>, text: &str, facts: &mut FileFacts, scope: usize) -> EngineResult<()> {
    if let Some(path) = node.child_by_field_name("path") {
        let raw = content(path, text);
        if let Some(path_value) = raw
            .strip_prefix('"')
            .and_then(|s| s.strip_suffix('"'))
            .or_else(|| raw.strip_prefix('`').and_then(|s| s.strip_suffix('`')))
        {
            let parts = path_value.split('/').map(str::to_owned).collect::<Vec<_>>();
            reference(
                facts,
                scope,
                RelationKind::Import,
                path,
                Lookup::Import {
                    path: parts.clone(),
                },
            )?;
            let alias = node
                .child_by_field_name("name")
                .map(|n| content(n, text).to_owned());
            if alias.as_deref() != Some("_") {
                facts.bindings.push(Binding {
                    scope,
                    name: alias
                        .clone()
                        .unwrap_or_else(|| parts.last().expect("import path").clone()),
                    target: BindingTarget::PackageImport { path: parts, alias },
                    visible_from: 0,
                    visibility: String::new(),
                });
            }
        } else {
            reference(facts, scope, RelationKind::Import, path, Lookup::Dynamic)?;
        }
    }
    Ok(())
}

fn function(
    node: Node<'_>,
    text: &str,
    facts: &mut FileFacts,
    mut scope: usize,
) -> EngineResult<usize> {
    if let Some(name) = node.child_by_field_name("name") {
        let qualified = if let Some(receiver) = node.child_by_field_name("receiver") {
            receiver_type(receiver, text).map(|owner| format!("{owner}::{}", content(name, text)))
        } else {
            Some(content(name, text).into())
        };
        let callable = qualified.is_some()
            && node.child_by_field_name("type_parameters").is_none()
            && node.child_by_field_name("body").is_some();
        let id = symbol(
            facts,
            scope,
            node,
            qualified.as_deref().unwrap_or(content(name, text)),
            if node.kind() == "function_declaration" {
                EntityKind::Function
            } else {
                EntityKind::Method
            },
            callable,
        )?;
        scope = add_scope(facts, scope, id, ScopeKind::Function);
    }
    Ok(scope)
}

fn call(node: Node<'_>, text: &str, facts: &mut FileFacts, scope: usize) -> EngineResult<()> {
    if let Some(function) = node.child_by_field_name("function") {
        let parts = match function.kind() {
            "identifier" => dotted(function, text),
            "selector_expression" => function
                .child_by_field_name("operand")
                .filter(|n| n.kind() == "identifier")
                .zip(function.child_by_field_name("field"))
                .map(|(a, b)| vec![content(a, text).into(), content(b, text).into()]),
            _ => None,
        };
        reference(
            facts,
            scope,
            RelationKind::Call,
            function,
            parts.map_or(Lookup::Dynamic, |parts| Lookup::Name { parts }),
        )?;
    }
    Ok(())
}

fn fields(node: Node<'_>, text: &str, facts: &mut FileFacts, scope: usize) -> EngineResult<()> {
    let names = children(node)
        .into_iter()
        .filter(|n| n.kind() == "field_identifier")
        .collect::<Vec<_>>();
    if names.is_empty() {
        if let Some(kind) = node
            .child_by_field_name("type")
            .or_else(|| node.named_child(0))
        {
            let raw = content(kind, text).trim_start_matches('*');
            if let Some(name) = raw.rsplit('.').next().filter(|n| !n.is_empty()) {
                symbol(facts, scope, node, name, EntityKind::Field, false)?;
            }
        }
    } else {
        for name in names {
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
    Ok(())
}
