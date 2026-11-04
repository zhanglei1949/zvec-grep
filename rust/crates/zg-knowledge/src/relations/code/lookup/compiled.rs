use super::{
    BTreeSet, BindingTarget, FileFacts, Found, Language, NodeKind, Reference, ResolutionStatus,
    Resolver, Result, module_path, target,
};
use crate::relations::{EntityKind, FactNode, Lookup, RelationKind};

fn package(file: &FileFacts) -> Option<&str> {
    file.nodes
        .iter()
        .find(|n| n.kind == NodeKind::Module && n.parent.as_ref() == Some(&file.scopes[0].owner))
        .map(|n| n.name.as_str())
}
impl Resolver<'_, '_> {
    pub(super) fn compiled_name(
        &mut self,
        file: &FileFacts,
        reference: &Reference,
        parts: &[String],
    ) -> Result<Found> {
        let Some(first) = parts.first() else {
            return Ok(Found::unknown("empty_name"));
        };
        if file.language == Language::Cpp && first == "::" {
            let names = &parts[1..];
            if names.is_empty() {
                return Ok(Found::unknown("empty_name"));
            }
            return self.c_name(file, reference, names, true);
        }
        let mut cursor = Some(reference.scope);
        while let Some(scope) = cursor {
            self.check()?;
            if !matches!(file.language, Language::Go | Language::C | Language::Cpp) || scope != 0 {
                let bindings = file
                    .bindings
                    .iter()
                    .filter(|b| {
                        b.scope == scope
                            && b.name == *first
                            && b.visible_from <= reference.span.start
                            && !matches!(b.target, BindingTarget::PackageImport { .. })
                            && !(reference.kind == RelationKind::Call
                                && b.visibility == "declaration")
                    })
                    .collect::<Vec<_>>();
                if !bindings.is_empty() {
                    return self.bindings(file, &bindings, &parts[1..], &module_path(file, scope));
                }
            }
            cursor = file.scopes[scope].parent;
        }
        match file.language {
            Language::Go => self.go_name(file, reference, parts),
            Language::Java => {
                let mut path = file.module.as_ref().expect("code module").path.clone();
                path.extend_from_slice(parts);
                let found = self.absolute(&path, &module_path(file, reference.scope))?;
                if (found.complete && found.targets.is_empty()
                    || found.status == ResolutionStatus::External)
                    && self.catalog.iter().any(|f| {
                        f.module
                            .as_ref()
                            .is_some_and(|m| !m.path.is_empty() && m.path.first() == parts.first())
                    })
                {
                    self.absolute(parts, &module_path(file, reference.scope))
                } else {
                    Ok(found)
                }
            }
            Language::C | Language::Cpp => self.c_name(file, reference, parts, false),
            _ => unreachable!("compiled resolver"),
        }
    }
    fn go_files<'a>(&'a self, path: &[String]) -> Vec<&'a FileFacts> {
        self.catalog
            .iter()
            .copied()
            .filter(|f| f.module.as_ref().is_some_and(|m| m.path == path))
            .collect()
    }
    pub(super) fn go_import(&mut self, _file: &FileFacts, path: &[String]) -> Result<Found> {
        self.check()?;
        let files = self.go_files(path);
        if files.is_empty() {
            return Ok(Found::external());
        }
        let names = files
            .iter()
            .filter_map(|f| package(f))
            .collect::<BTreeSet<_>>();
        if names.len() != 1 {
            return Ok(Found::unknown("conflicting_package_declarations"));
        }
        if files.iter().any(|f| !f.complete) {
            return Ok(Found::unknown("package_build_context_incomplete"));
        }
        // A package has multiple declarations. Its import projection uses the
        // first source path, after proving every selected declaration agrees.
        let representative = files
            .into_iter()
            .min_by_key(|f| &f.version.path)
            .expect("nonempty package");
        let node = representative
            .nodes
            .iter()
            .find(|n| n.kind == NodeKind::Module)
            .expect("package declaration");
        Ok(Found::node(target(representative, node)))
    }
    fn go_name(
        &mut self,
        file: &FileFacts,
        _reference: &Reference,
        parts: &[String],
    ) -> Result<Found> {
        let mut found = Found::absent();
        let own = file.module.as_ref().expect("code module").path.clone();
        for other in self.catalog.iter().copied().filter(|f| {
            f.module.as_ref().is_some_and(|m| m.path == own) && package(f) == package(file)
        }) {
            if !other.complete {
                found.merge(Found::unknown("package_build_context_incomplete"));
                continue;
            }
            let bindings = other
                .bindings
                .iter()
                .filter(|b| {
                    b.scope == 0
                        && b.name == parts[0]
                        && !matches!(b.target, BindingTarget::PackageImport { .. })
                })
                .collect::<Vec<_>>();
            if !bindings.is_empty() {
                found.merge(self.bindings(other, &bindings, &parts[1..], &own)?);
            }
        }
        if !found.targets.is_empty() || !found.complete {
            return Ok(found);
        }
        for binding in &file.bindings {
            let BindingTarget::PackageImport { path, alias } = &binding.target else {
                continue;
            };
            if alias.as_deref() == Some(".") {
                continue;
            }
            let imported = self
                .catalog
                .iter()
                .copied()
                .filter(|f| f.module.as_ref().is_some_and(|m| &m.path == path))
                .collect::<Vec<_>>();
            let matches = alias.as_ref().map_or_else(
                || {
                    imported
                        .iter()
                        .any(|f| package(f) == Some(parts[0].as_str()))
                },
                |a| a == &parts[0],
            );
            if !matches {
                continue;
            }
            if imported.is_empty() {
                return Ok(Found::external());
            }
            let Some(name) = parts
                .get(1)
                .filter(|p| p.chars().next().is_some_and(char::is_uppercase))
            else {
                return Ok(Found::unknown("package_member_not_exported"));
            };
            for other in imported {
                if !other.complete {
                    found.merge(Found::unknown("package_build_context_incomplete"));
                    continue;
                }
                let bindings = other
                    .bindings
                    .iter()
                    .filter(|b| b.scope == 0 && &b.name == name)
                    .collect::<Vec<_>>();
                if !bindings.is_empty() {
                    found.merge(self.bindings(other, &bindings, &parts[2..], path)?);
                }
            }
        }
        if found.targets.is_empty() && found.complete && parts.len() > 1 {
            // Concrete Go method expressions are declarations, not receiver
            // inference. Methods themselves may be declared in another file.
            let types = self
                .catalog
                .iter()
                .copied()
                .filter(|f| f.module.as_ref().is_some_and(|m| m.path == own))
                .flat_map(|f| f.nodes.iter())
                .filter(|n| n.name == parts[0] && n.entity_kind.is_type())
                .count();
            if types == 1 {
                for other in self
                    .catalog
                    .iter()
                    .copied()
                    .filter(|f| f.module.as_ref().is_some_and(|m| m.path == own))
                {
                    for node in other.nodes.iter().filter(|n| {
                        n.qualified_name == parts && n.entity_kind == EntityKind::Method
                    }) {
                        found.merge(Found::node(target(other, node)));
                    }
                }
            }
        }
        Ok(found)
    }
    pub(crate) fn include(
        &mut self,
        file: &FileFacts,
        path: &str,
        system: bool,
        pinned: Option<&str>,
    ) -> Result<Found> {
        self.check()?;
        let path = if let Some(pinned) = pinned {
            Some(pinned.to_owned())
        } else if !system {
            relative(
                file.version
                    .path
                    .rsplit_once('/')
                    .map_or("", |(parent, _)| parent),
                path,
            )
        } else {
            None
        };
        let Some(path) = path else {
            return Ok(Found::external());
        };
        let Some(other) = self.catalog.iter().find(|f| f.version.path == path) else {
            return Ok(Found::external());
        };
        if !other.complete {
            return Ok(Found::unknown("incomplete_include"));
        }
        Ok(Found::node(target(other, &other.nodes[0])))
    }
    fn c_name(
        &mut self,
        file: &FileFacts,
        reference: &Reference,
        parts: &[String],
        absolute: bool,
    ) -> Result<Found> {
        let mut sources = BTreeSet::new();
        let mut pending = vec![file];
        while let Some(source) = pending.pop() {
            self.check()?;
            if !sources.insert(source.version.path.as_str()) {
                continue;
            }
            for included in &source.references {
                if let Lookup::Include {
                    path,
                    system,
                    target: pinned,
                } = &included.lookup
                {
                    let targets = self.include(source, path, *system, pinned.as_deref())?;
                    for target in targets.targets {
                        if let Some(next) =
                            self.catalog.iter().find(|f| f.version.path == target.path)
                        {
                            pending.push(next);
                        }
                    }
                }
            }
        }
        let mut paths = Vec::new();
        if !absolute && file.language == Language::Cpp && reference.kind == RelationKind::Extends {
            let mut scope = Some(reference.scope);
            while let Some(index) = scope {
                if file.scopes[index].kind == crate::relations::ScopeKind::Module {
                    let owner = &file.scopes[index].owner;
                    if let Some(node) = file.nodes.iter().find(|n| &n.id == owner) {
                        let mut prefix = node.qualified_name.clone();
                        while !prefix.is_empty() {
                            let mut qualified = prefix.clone();
                            qualified.extend_from_slice(parts);
                            paths.push(qualified);
                            prefix.pop();
                        }
                    }
                    break;
                }
                scope = file.scopes[index].parent;
            }
        }
        paths.push(parts.to_vec());
        for qualified in paths {
            let mut found = Found::absent();
            for other in self
                .catalog
                .iter()
                .copied()
                .filter(|f| sources.contains(f.version.path.as_str()))
            {
                if !other.complete {
                    found.merge(Found::unknown("incomplete_compilation_unit"));
                    continue;
                }
                if other.bindings.iter().any(|b| {
                    b.scope == 0 && b.name == parts[0] && b.target == BindingTarget::Dynamic
                }) {
                    found.merge(Found::unknown("macro_or_dynamic_binding"));
                    continue;
                }
                for node in other
                    .nodes
                    .iter()
                    .filter(|n| n.qualified_name == qualified && n.kind != NodeKind::File)
                {
                    let binding = other
                        .bindings
                        .iter()
                        .find(|b| matches!(&b.target,BindingTarget::Node{id} if id==&node.id));
                    if binding.is_none() {
                        found.merge(Found::unknown("conditional_declaration"));
                        continue;
                    }
                    if !super::visible(
                        other,
                        binding.expect("checked binding"),
                        &module_path(file, reference.scope),
                    ) {
                        continue;
                    }
                    if reference.kind == RelationKind::Extends && !node.path_addressable {
                        continue;
                    }
                    if is_declaration(other, node) && reference.kind == RelationKind::Call {
                        continue;
                    }
                    found.merge(Found::node(target(other, node)));
                }
            }
            if !found.targets.is_empty() || !found.complete {
                return Ok(found);
            }
        }

        Ok(Found::absent())
    }
}
fn is_declaration(file: &FileFacts, node: &FactNode) -> bool {
    file.bindings.iter().any(|b| {
        matches!(&b.target,BindingTarget::Node{id} if id == &node.id)
            && b.visibility == "declaration"
    })
}
pub(crate) fn relative(base: &str, value: &str) -> Option<String> {
    if value.starts_with('/') || value.contains(['\\', ':', '\0']) {
        return None;
    }
    let mut parts = base
        .split('/')
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>();
    for part in value.split('/') {
        match part {
            "" | "." => (),
            ".." => {
                parts.pop()?;
            }
            p => parts.push(p),
        }
    }
    (!parts.is_empty()).then(|| parts.join("/"))
}
