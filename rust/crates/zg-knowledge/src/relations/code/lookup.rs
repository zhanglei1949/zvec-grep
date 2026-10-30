mod implementation;
use crate::{
    Error, Result,
    processing::ProcessingContext,
    relations::{
        Binding, BindingTarget, FileFacts, Language, NodeKind, Reference, ResolutionStatus,
        ScopeKind, Target, target,
    },
};
use std::collections::BTreeSet;

pub(super) struct Found {
    pub targets: Vec<Target>,
    pub complete: bool,
    pub status: ResolutionStatus,
    pub reason: &'static str,
}
impl Found {
    fn absent() -> Self {
        Self {
            targets: Vec::new(),
            complete: true,
            status: ResolutionStatus::Unresolved,
            reason: "no_static_binding",
        }
    }
    pub(super) fn unknown(reason: &'static str) -> Self {
        Self {
            complete: false,
            reason,
            ..Self::absent()
        }
    }
    fn external() -> Self {
        Self {
            status: ResolutionStatus::External,
            ..Self::unknown("external_module")
        }
    }
    fn node(t: Target) -> Self {
        Self {
            targets: vec![t],
            reason: "lexical_binding",
            ..Self::absent()
        }
    }
    fn merge(&mut self, other: Self) {
        let pristine =
            self.complete && self.targets.is_empty() && self.reason == "no_static_binding";
        if pristine {
            self.status = other.status;
            self.reason = other.reason;
        } else if other.status != ResolutionStatus::External {
            self.status = ResolutionStatus::Unresolved;
        }
        self.complete &= other.complete;
        self.targets.extend(other.targets);
        if !other.complete {
            self.reason = other.reason;
        }
    }
}

pub(super) struct Resolver<'a, 'b> {
    catalog: &'a [&'a FileFacts],
    context: &'b ProcessingContext,
    visits: &'b mut usize,
    active: BTreeSet<(String, usize, String)>,
}
impl<'a, 'b> Resolver<'a, 'b> {
    pub fn new(
        catalog: &'a [&'a FileFacts],
        context: &'b ProcessingContext,
        visits: &'b mut usize,
    ) -> Self {
        Self {
            catalog,
            context,
            visits,
            active: BTreeSet::new(),
        }
    }
    fn check(&mut self) -> Result<()> {
        *self.visits = self.visits.saturating_add(1);
        self.context.check_output(*self.visits)
    }

    pub fn name(
        &mut self,
        file: &FileFacts,
        reference: &Reference,
        parts: &[String],
    ) -> Result<Found> {
        self.name_at(file, reference.scope, reference.span.start, parts)
    }

    fn name_at(
        &mut self,
        file: &FileFacts,
        start_scope: usize,
        position: usize,
        parts: &[String],
    ) -> Result<Found> {
        let Some(first) = parts.first() else {
            return Ok(Found::unknown("empty_name"));
        };
        let origin = module_path(file, start_scope);
        if file.language == Language::Rust && matches!(first.as_str(), "crate" | "self" | "super") {
            return self.path(file, start_scope, parts, &origin);
        }
        if file.language == Language::Rust && first == "Self" {
            return self.self_name(file, start_scope, &parts[1..], &origin);
        }
        let mut scope = Some(start_scope);
        let mut crossed_function = false;
        let mut seen = BTreeSet::new();
        while let Some(index) = scope {
            self.check()?;
            if !seen.insert(index) {
                return Err(Error::Provider("cyclic name routing"));
            }
            let current = file
                .scopes
                .get(index)
                .ok_or(Error::Provider("invalid scope"))?;
            if file.language == Language::Rust
                && matches!(
                    current.kind,
                    ScopeKind::InherentImpl | ScopeKind::UnsupportedImpl | ScopeKind::Class
                )
                && file.bindings.iter().any(|b| {
                    b.scope == index && &b.name == first && b.target == BindingTarget::Dynamic
                })
            {
                return Ok(Found::unknown("shadowed_or_dynamic_binding"));
            }
            if !(matches!(
                current.kind,
                ScopeKind::Class
                    | ScopeKind::InherentImpl
                    | ScopeKind::UnsupportedImpl
                    | ScopeKind::TypeAlias
            ) && (file.language == Language::Rust || crossed_function))
            {
                let bindings = file
                    .bindings
                    .iter()
                    .filter(|b| b.scope == index && &b.name == first && b.visible_from <= position)
                    .collect::<Vec<_>>();
                if let Some(target_scope) = bindings.iter().find_map(|b| match b.target {
                    BindingTarget::Redirect { scope } => Some(scope),
                    _ => None,
                }) {
                    scope = Some(target_scope);
                    continue;
                }
                if !bindings.is_empty() {
                    return self.bindings(file, &bindings, &parts[1..], &origin);
                }
                if file.language == Language::Rust {
                    let found = self.globs(file, index, parts, &origin)?;
                    if !found.complete || !found.targets.is_empty() {
                        return Ok(found);
                    }
                }
            }
            crossed_function |=
                matches!(current.kind, ScopeKind::Function | ScopeKind::Comprehension);
            scope = current.parent;
        }
        if file.language == Language::Rust && parts.len() > 1 {
            return self.path(file, start_scope, parts, &origin);
        }
        Ok(Found::absent())
    }

    fn bindings(
        &mut self,
        file: &FileFacts,
        bindings: &[&Binding],
        rest: &[String],
        origin: &[String],
    ) -> Result<Found> {
        if file.language == Language::Rust && !rest.is_empty() {
            let base = self.bindings(file, bindings, &[], origin)?;
            return self.tail(base, rest, origin);
        }
        let mut found = Found {
            status: ResolutionStatus::External,
            ..Found::absent()
        };
        for binding in bindings {
            self.check()?;
            let next = match &binding.target {
                BindingTarget::Dynamic => Found::unknown("shadowed_or_dynamic_binding"),
                BindingTarget::Node { id } => {
                    let n = file
                        .nodes
                        .iter()
                        .find(|n| &n.id == id)
                        .ok_or(Error::Provider("missing binding node"))?;
                    self.tail(Found::node(target(file, n)), rest, origin)?
                }
                BindingTarget::Import { path } => {
                    // `import a.b` binds a, but explicitly loads a.b even when
                    // a is a namespace package with no indexed __init__ file.
                    let mut full = path.clone();
                    full.extend_from_slice(rest);
                    let loaded_prefix = file.language == Language::Python && file.references.iter().any(|r| {
                        r.scope == binding.scope && matches!(&r.lookup, super::super::Lookup::Import {path: imported} if imported.len() > path.len() && imported.starts_with(path) && full.starts_with(imported))
                    });
                    if loaded_prefix {
                        self.path(
                            file,
                            binding.scope,
                            &full,
                            &module_path(file, binding.scope),
                        )?
                    } else {
                        let base = self.path(
                            file,
                            binding.scope,
                            path,
                            &module_path(file, binding.scope),
                        )?;
                        self.tail(base, rest, origin)?
                    }
                }
                BindingTarget::GlobImport { .. } | BindingTarget::Redirect { .. } => {
                    return Err(Error::Provider("invalid named binding"));
                }
            };
            found.merge(next);
        }
        Ok(found)
    }

    fn tail(&mut self, mut base: Found, rest: &[String], origin: &[String]) -> Result<Found> {
        if rest.is_empty() || !base.complete {
            return Ok(base);
        }
        base.targets
            .sort_by(|a, b| (&a.path, &a.node_id).cmp(&(&b.path, &b.node_id)));
        base.targets.dedup();
        if file_is_rust(self.catalog) && base.targets.len() > 1 {
            return Ok(Found {
                status: ResolutionStatus::Ambiguous,
                ..Found::unknown("ambiguous_path_prefix")
            });
        }
        let mut found = Found::absent();
        for t in base.targets {
            let file = self
                .catalog
                .iter()
                .find(|f| f.version.path == t.path)
                .copied()
                .ok_or(Error::Provider("missing target source"))?;
            let n = file
                .nodes
                .iter()
                .find(|n| n.id == t.node_id)
                .ok_or(Error::Provider("missing target node"))?;
            if file.language == Language::Rust
                && n.entity_kind == crate::relations::EntityKind::Type
            {
                let nominal = self.nominal(Found::node(t.clone()))?;
                found.merge(self.associated(nominal, rest, origin)?);
                continue;
            }
            if let Some(scope) = file.scopes.iter().position(|s| {
                s.owner == n.id
                    && (n.path_addressable || n.kind != NodeKind::Module)
                    && matches!(
                        s.kind,
                        ScopeKind::Module | ScopeKind::Enum | ScopeKind::Class
                    )
            }) {
                found.merge(self.member(file, scope, rest, origin)?);
            } else if n.kind == NodeKind::Module {
                let mut path = file.module.as_ref().expect("code module").path.clone();
                path.extend(n.qualified_name.clone());
                path.extend_from_slice(rest);
                found.merge(self.absolute(&path, origin)?);
            }
        }
        Ok(found)
    }

    fn glob(
        &mut self,
        file: &FileFacts,
        scope: usize,
        path: &[String],
        parts: &[String],
        origin: &[String],
    ) -> Result<Found> {
        let base = self.path(file, scope, path, &module_path(file, scope))?;
        let mut found = self.tail(base, parts, origin)?;
        if !found.complete && found.status == ResolutionStatus::External {
            found.status = ResolutionStatus::Unresolved;
            found.reason = "unknown_glob_binding";
        }
        Ok(found)
    }

    fn globs(
        &mut self,
        file: &FileFacts,
        scope: usize,
        parts: &[String],
        origin: &[String],
    ) -> Result<Found> {
        let mut found = Found::absent();
        for binding in file.bindings.iter().filter(|b| b.scope == scope) {
            if let BindingTarget::GlobImport { path } = &binding.target {
                found.merge(self.glob(file, scope, path, &parts[..1], origin)?);
            }
        }
        self.tail(found, &parts[1..], origin)
    }

    fn member(
        &mut self,
        file: &FileFacts,
        scope: usize,
        parts: &[String],
        origin: &[String],
    ) -> Result<Found> {
        self.check()?;
        let Some(first) = parts.first() else {
            return Ok(Found::absent());
        };
        let key = (file.version.path.clone(), scope, parts.join("::"));
        if self.active.len() >= 64 {
            return Err(Error::ResourceExhausted);
        }
        if !self.active.insert(key.clone()) {
            return Ok(Found::unknown("cyclic_import"));
        }
        let result = self.member_inner(file, scope, first, &parts[1..], origin);
        self.active.remove(&key);
        result
    }

    fn member_inner(
        &mut self,
        file: &FileFacts,
        scope: usize,
        first: &str,
        rest: &[String],
        origin: &[String],
    ) -> Result<Found> {
        if !file.complete {
            return Ok(Found::unknown("incomplete_target"));
        }
        let named = file
            .bindings
            .iter()
            .filter(|b| {
                b.scope == scope
                    && b.name == first
                    && !matches!(b.target, BindingTarget::Redirect { .. })
            })
            .collect::<Vec<_>>();
        if !named.is_empty() {
            let bindings = named
                .into_iter()
                .filter(|b| visible(file, b, origin))
                .collect::<Vec<_>>();
            // Privacy cannot expose a lower-priority glob with the same name.
            if bindings.is_empty() {
                return Ok(Found::absent());
            }
            return self.bindings(file, &bindings, rest, origin);
        }
        let mut found = Found::absent();
        let mut external_source = false;
        for b in file
            .bindings
            .iter()
            .filter(|b| b.scope == scope && visible(file, b, origin))
        {
            if let BindingTarget::GlobImport { path } = &b.target {
                let Some(normalized) = normalize(file, scope, path) else {
                    return Ok(Found::unknown("invalid_relative_module"));
                };
                let mut own = module_path(file, scope);
                own.push(first.into());
                if normalized.starts_with(&own) {
                    external_source = true;
                    continue;
                }
                let parts = vec![first.into()];
                found.merge(self.glob(file, scope, path, &parts, origin)?);
            }
        }
        if external_source && found.complete && found.targets.is_empty() {
            return Ok(Found::external());
        }
        self.tail(found, rest, origin)
    }

    pub fn path(
        &mut self,
        file: &FileFacts,
        scope: usize,
        path: &[String],
        origin: &[String],
    ) -> Result<Found> {
        let Some(path) = normalize(file, scope, path) else {
            return Ok(Found::unknown("invalid_relative_module"));
        };
        self.absolute(&path, origin)
    }

    fn absolute(&mut self, path: &[String], origin: &[String]) -> Result<Found> {
        self.check()?;
        let mut spaces = Vec::new();
        let mut longest = 0;
        for file in self.catalog {
            self.context.check()?;
            for (index, scope) in file.scopes.iter().enumerate().filter(|(_, s)| {
                s.kind == ScopeKind::Module
                    || file.language == Language::Python && s.kind == ScopeKind::Class
            }) {
                let owner = file
                    .nodes
                    .iter()
                    .find(|n| n.id == scope.owner)
                    .ok_or(Error::Provider("missing scope owner"))?;
                if index != 0 && owner.kind == NodeKind::Module && !owner.path_addressable {
                    continue;
                }
                let mut prefix = file.module.as_ref().expect("code module").path.clone();
                prefix.extend(owner.qualified_name.clone());
                if path.starts_with(&prefix) && prefix.len() >= longest {
                    if prefix.len() > longest {
                        spaces.clear();
                        longest = prefix.len();
                    }
                    spaces.push((*file, index, prefix));
                }
            }
        }
        if spaces.is_empty() {
            return Ok(Found::external());
        }
        let mut found = Found::absent();
        for (file, scope, prefix) in spaces {
            if !namespace_visible(file, scope, origin) {
                continue;
            }
            if path.len() == prefix.len() {
                if file.complete {
                    let owner = &file.scopes[scope].owner;
                    let n = file
                        .nodes
                        .iter()
                        .find(|n| &n.id == owner)
                        .ok_or(Error::Provider("missing namespace"))?;
                    found.merge(Found::node(target(file, n)));
                } else {
                    found.merge(Found::unknown("incomplete_target"));
                }
            } else {
                found.merge(self.member(file, scope, &path[prefix.len()..], origin)?);
            }
        }
        if longest == 0 && found.complete && found.targets.is_empty() {
            return Ok(Found::external());
        }
        Ok(found)
    }
}

pub(super) fn module_path(file: &FileFacts, scope: usize) -> Vec<String> {
    let mut base = file.module.as_ref().expect("code module").path.clone();
    let mut cursor = Some(scope);
    while let Some(i) = cursor {
        let current = &file.scopes[i];
        if current.kind == ScopeKind::Module {
            if let Some(node) = file
                .nodes
                .iter()
                .find(|n| n.id == current.owner && n.kind == NodeKind::Module)
            {
                base.extend(node.qualified_name.clone());
            }
            break;
        }
        cursor = current.parent;
    }
    base
}
fn normalize(file: &FileFacts, scope: usize, path: &[String]) -> Option<Vec<String>> {
    let module = file.module.as_ref()?;
    let mut path = path.to_vec();
    if file.language == Language::Rust {
        match path.first().map(String::as_str) {
            Some("crate") => {
                path.remove(0);
            }
            Some("self" | "super") => {
                let mut base = module_path(file, scope);
                if path.first().is_some_and(|p| p == "self") {
                    path.remove(0);
                }
                while path.first().is_some_and(|p| p == "super") {
                    path.remove(0);
                    base.pop()?;
                }
                base.extend(path);
                path = base;
            }
            _ => (),
        }
    } else if path.first().is_some_and(|p| p == ".") {
        let mut base = module.path.clone();
        if !module.is_package {
            base.pop();
        }
        path.remove(0);
        while path.first().is_some_and(|p| p == ".") {
            path.remove(0);
            base.pop()?;
        }
        base.extend(path);
        path = base;
    }
    Some(path)
}
fn visible(file: &FileFacts, binding: &Binding, origin: &[String]) -> bool {
    if file.language != Language::Rust {
        return true;
    }
    let mut base = module_path(file, binding.scope);
    match binding.visibility.as_str() {
        "pub" | "pub(crate)" => true,
        "pub(super)" => {
            base.pop();
            origin.starts_with(&base)
        }
        "" | "pub(self)" => origin.starts_with(&base),
        v => v
            .strip_prefix("pub(in")
            .and_then(|v| v.strip_suffix(')'))
            .and_then(|v| {
                normalize(
                    file,
                    binding.scope,
                    &v.split("::").map(str::to_owned).collect::<Vec<_>>(),
                )
            })
            .is_some_and(|p| origin.starts_with(&p)),
    }
}
fn namespace_visible(file: &FileFacts, scope: usize, origin: &[String]) -> bool {
    let mut cursor = Some(scope);
    while let Some(i) = cursor {
        let owner = &file.scopes[i].owner;
        if file.bindings.iter().any(|b| {
            matches!(&b.target, BindingTarget::Node {id} if id == owner)
                && !visible(file, b, origin)
        }) {
            return false;
        }
        cursor = file.scopes[i].parent;
    }
    true
}

fn file_is_rust(catalog: &[&FileFacts]) -> bool {
    catalog
        .first()
        .is_some_and(|f| f.language == Language::Rust)
}
