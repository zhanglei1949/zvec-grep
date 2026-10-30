//! Explicit nominal paths only; no receiver, trait or constraint inference.
use super::{Found, Resolver};
use crate::{
    Error, Result,
    relations::{BindingTarget, EntityKind, FileFacts, ResolutionStatus, ScopeKind},
};

impl Resolver<'_, '_> {
    pub(super) fn self_name(
        &mut self,
        file: &FileFacts,
        scope: usize,
        rest: &[String],
        origin: &[String],
    ) -> Result<Found> {
        let mut cursor = Some(scope);
        let mut function_depth = 0;
        while let Some(index) = cursor {
            self.check()?;
            let current = &file.scopes[index];
            if matches!(
                current.kind,
                ScopeKind::InherentImpl | ScopeKind::UnsupportedImpl | ScopeKind::Class
            ) {
                if current.kind != ScopeKind::InherentImpl || function_depth > 1 {
                    return Ok(Found::unknown("unsupported_self_context"));
                }
                let base = self.impl_type(file, index)?;
                return self.tail(base, rest, origin);
            }
            function_depth += usize::from(current.kind == ScopeKind::Function);
            cursor = current.parent;
        }
        Ok(Found::unknown("missing_self_context"))
    }
    fn type_path(&mut self, file: &FileFacts, scope: usize, path: &[String]) -> Result<Found> {
        self.name_at(file, scope, usize::MAX, path)
    }
    pub(super) fn impl_type(&mut self, file: &FileFacts, scope: usize) -> Result<Found> {
        let parent = file.scopes[scope]
            .parent
            .ok_or(Error::Provider("missing type context"))?;
        self.check()?;
        let key = (file.version.path.clone(), scope, "@impl_type".into());
        if self.active.len() >= 64 {
            return Err(Error::ResourceExhausted);
        }
        if !self.active.insert(key.clone()) {
            return Ok(Found::unknown("cyclic_impl_type"));
        }
        let result = (|| match file
            .bindings
            .iter()
            .find(|b| b.scope == scope && b.name == "Self")
            .map(|b| &b.target)
        {
            Some(BindingTarget::Import { path }) => {
                let base = self.type_path(file, parent, path)?;
                self.nominal(base)
            }
            Some(BindingTarget::Dynamic) => Ok(Found::unknown("unsupported_type_path")),
            _ => Err(Error::Provider("missing type context binding")),
        })();
        self.active.remove(&key);
        result
    }
    pub(super) fn nominal(&mut self, mut base: Found) -> Result<Found> {
        self.check()?;
        base.targets
            .sort_by(|a, b| (&a.path, &a.node_id).cmp(&(&b.path, &b.node_id)));
        base.targets.dedup();
        if !base.complete {
            return Ok(base);
        }
        let [target] = base.targets.as_slice() else {
            return Ok(if base.targets.is_empty() {
                base
            } else {
                Found {
                    status: ResolutionStatus::Ambiguous,
                    ..Found::unknown("ambiguous_type")
                }
            });
        };
        let file = self
            .catalog
            .iter()
            .find(|f| f.version.path == target.path)
            .copied()
            .ok_or(Error::Provider("missing type file"))?;
        let node = file
            .nodes
            .iter()
            .find(|n| n.id == target.node_id)
            .ok_or(Error::Provider("missing type"))?;
        if node.entity_kind != EntityKind::Type {
            return Ok(Found::unknown("not_nominal_type"));
        }
        let Some(alias) = file
            .scopes
            .iter()
            .position(|s| s.kind == ScopeKind::TypeAlias && s.owner == node.id)
        else {
            return Ok(base);
        };
        let key = (file.version.path.clone(), alias, "@type_alias".into());
        if self.active.len() >= 64 {
            return Err(Error::ResourceExhausted);
        }
        if !self.active.insert(key.clone()) {
            return Ok(Found::unknown("cyclic_type_alias"));
        }
        let result = self.impl_type(file, alias);
        self.active.remove(&key);
        result
    }
    pub(super) fn associated(
        &mut self,
        base: Found,
        rest: &[String],
        origin: &[String],
    ) -> Result<Found> {
        if !base.complete || base.targets.is_empty() {
            return Ok(base);
        }
        let [target] = base.targets.as_slice() else {
            return Ok(Found::unknown("ambiguous_type"));
        };
        let Some(first) = rest.first() else {
            return Ok(base);
        };
        let mut found = Found::absent();
        // Enum variants and associated functions share a type namespace.
        let file = self
            .catalog
            .iter()
            .find(|f| f.version.path == target.path)
            .copied()
            .ok_or(Error::Provider("missing type source"))?;
        if let Some(scope) = file
            .scopes
            .iter()
            .position(|s| s.owner == target.node_id && s.kind == ScopeKind::Enum)
        {
            found.merge(self.member(file, scope, rest, origin)?);
        }
        for file in self.catalog {
            for (scope, current) in file.scopes.iter().enumerate() {
                self.check()?;
                if !matches!(
                    current.kind,
                    ScopeKind::InherentImpl | ScopeKind::UnsupportedImpl
                ) || !file
                    .bindings
                    .iter()
                    .any(|b| b.scope == scope && &b.name == first)
                {
                    continue;
                }
                let owner = self.impl_type(file, scope)?;
                if !owner.complete {
                    found.merge(Found::unknown("unknown_impl_owner"));
                    continue;
                }
                if owner.targets.as_slice() != std::slice::from_ref(target) {
                    continue;
                }
                if current.kind == ScopeKind::UnsupportedImpl || !file.complete {
                    found.merge(Found::unknown("conditional_impl_unsupported"));
                } else {
                    found.merge(self.member(file, scope, rest, origin)?);
                }
            }
        }
        Ok(found)
    }
}
