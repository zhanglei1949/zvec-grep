//! Explicit workspace-relative module mapping. No Cargo/Python discovery.
use super::relations::{CodeModule, Language};
use crate::{EngineError, EngineResult, domain::FileFormat};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeSet,
    path::{Component, Path, PathBuf},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PackageLayout {
    Flat,
    Src,
    ModRs,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModuleRoot {
    pub root: PathBuf,
    pub language: Language,
    pub namespace: String,
    pub module_prefix: Vec<String>,
    pub layout: PackageLayout,
}
#[derive(Clone, Debug)]
pub struct ModuleRoots {
    roots: Vec<ModuleRoot>,
    fingerprint: String,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ModuleMapping {
    Mapped(CodeModule),
    Missing,
    LanguageMismatch,
}
impl ModuleRoots {
    /// Validate normalized relative roots and canonicalize configuration order.
    /// The host separately checks real-path workspace containment before use.
    ///
    /// # Errors
    /// Rejects absolute/parent paths, duplicate roots or namespaces, Markdown
    /// roots and incompatible language layouts.
    pub fn new(mut roots: Vec<ModuleRoot>) -> EngineResult<Self> {
        let mut namespaces = BTreeSet::new();
        let mut locations = BTreeSet::new();
        for r in &roots {
            if r.namespace.trim().is_empty()
                || !namespaces.insert(r.namespace.clone())
                || !locations.insert(r.root.clone())
                || r.language == Language::Markdown
                || r.module_prefix.iter().any(String::is_empty)
                || r.root.is_absolute()
                || r.root
                    .components()
                    .any(|c| !matches!(c, Component::Normal(_)))
                || r.root.to_str().is_none()
                || (r.language == Language::Rust && r.layout == PackageLayout::Src)
                || (r.language == Language::Python && r.layout == PackageLayout::ModRs)
            {
                return Err(EngineError::invalid_argument(
                    "invalid or ambiguous graph module roots",
                ));
            }
        }
        roots.sort_by(|a, b| (&a.root, &a.namespace).cmp(&(&b.root, &b.namespace)));
        let fingerprint =
            zg_knowledge::processing::stable_digest(&roots).map_err(super::map_error)?;
        Ok(Self { roots, fingerprint })
    }
    #[must_use]
    pub fn fingerprint(&self) -> &str {
        &self.fingerprint
    }
    /// Check real paths before attaching the mapping to a host operation.
    ///
    /// # Errors
    /// Rejects absent/non-directory roots and symlinks escaping the workspace.
    pub fn validate_workspace(&self, workspace: &Path) -> EngineResult<()> {
        let invalid = || {
            EngineError::invalid_argument("graph module root is outside workspace or unavailable")
        };
        let workspace = workspace.canonicalize().map_err(|_| invalid())?;
        for root in &self.roots {
            let resolved = workspace
                .join(&root.root)
                .canonicalize()
                .map_err(|_| invalid())?;
            if !resolved.is_dir() || !resolved.starts_with(&workspace) {
                return Err(invalid());
            }
        }
        Ok(())
    }
    /// Map a normalized selected source using its actual host language.
    ///
    /// # Errors
    /// Rejects malformed paths or extensions; missing/mismatched roots remain
    /// explicit outcomes and cannot invent a unique code namespace.
    pub fn map(&self, path: &str, language: Language) -> EngineResult<ModuleMapping> {
        // Validate the same normalized path contract as facts before root matching.
        if path.is_empty()
            || path.contains(['\\', ':', '\0'])
            || path.split('/').any(|c| matches!(c, "" | "." | ".."))
        {
            return Err(EngineError::invalid_argument("invalid module source path"));
        }
        let path = Path::new(path);
        let Some(root) = self
            .roots
            .iter()
            .filter(|r| path.starts_with(&r.root))
            .max_by_key(|r| r.root.components().count())
        else {
            return Ok(ModuleMapping::Missing);
        };
        if root.language != language {
            return Ok(ModuleMapping::LanguageMismatch);
        }
        let relative = path
            .strip_prefix(&root.root)
            .map_err(|_| EngineError::invalid_argument("module outside root"))?;
        let relative = if root.layout == PackageLayout::Src {
            relative.strip_prefix("src").unwrap_or(relative)
        } else {
            relative
        };
        let mut components: Vec<String> = relative
            .iter()
            .map(|c| {
                c.to_str()
                    .map(str::to_owned)
                    .ok_or_else(|| EngineError::invalid_argument("non UTF-8 module path"))
            })
            .collect::<EngineResult<_>>()?;
        let file = components
            .pop()
            .ok_or_else(|| EngineError::invalid_argument("module file missing"))?;
        let format = if language == Language::Rust {
            FileFormat::Rust
        } else {
            FileFormat::Python
        };
        // Share the host catalog so Python .pyw/.pyi files accepted for text
        // indexing cannot fail solely because graph capture is enabled.
        let (stem, _) = file
            .rsplit_once('.')
            .filter(|(_, extension)| format.extensions().contains(extension))
            .ok_or_else(|| EngineError::invalid_argument("module extension mismatch"))?;
        let is_package = language == Language::Python && stem == "__init__";
        if !(is_package || language == Language::Rust && matches!(stem, "lib" | "main" | "mod")) {
            components.push(stem.to_owned());
        }
        let mut module_path = root.module_prefix.clone();
        module_path.extend(components);
        Ok(ModuleMapping::Mapped(CodeModule {
            namespace: root.namespace.clone(),
            path: module_path,
            is_package,
        }))
    }
}
