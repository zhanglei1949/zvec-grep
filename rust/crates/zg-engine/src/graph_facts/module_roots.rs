//! Explicit workspace-relative module mapping. No Cargo/Python discovery.
pub(crate) mod configuration;
use super::relations::{CodeModule, Language};
use crate::{EngineError, EngineResult, domain::FileFormat};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
};

pub use crate::api::index::graph::PackageLayout;
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
    configuration: Option<std::sync::Arc<configuration::Configuration>>,
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
            if !crate::api::index::graph::valid_module_root(
                &r.root,
                r.language,
                &r.namespace,
                &r.module_prefix,
                r.layout,
            ) || !namespaces.insert(r.namespace.clone())
                || !locations.insert(r.root.clone())
            {
                return Err(EngineError::invalid_argument(
                    "invalid or ambiguous graph module roots",
                ));
            }
        }
        roots.sort_by(|a, b| (&a.root, &a.namespace).cmp(&(&b.root, &b.namespace)));
        let fingerprint =
            zg_knowledge::processing::stable_digest(&roots).map_err(super::map_error)?;
        Ok(Self {
            roots,
            fingerprint,
            configuration: None,
        })
    }
    #[must_use]
    pub fn fingerprint(&self) -> &str {
        &self.fingerprint
    }
    #[cfg(any(test, feature = "knowledge-graph-graphdb"))]
    pub(crate) fn configured(mut self, workspace: &Path) -> EngineResult<Self> {
        self.validate_workspace(workspace)?;
        let configuration = configuration::Configuration::read(workspace, &self.roots)?;
        for root in &mut self.roots {
            if let Some(prefix) = configuration.prefixes.get(&root.root) {
                root.module_prefix.clone_from(prefix);
            }
        }
        self.fingerprint =
            zg_knowledge::processing::stable_digest(&(&self.roots, &configuration.inputs))
                .map_err(super::map_error)?;
        self.configuration = Some(std::sync::Arc::new(configuration));
        Ok(self)
    }
    pub(crate) fn inputs(&self) -> &[configuration::ConfigurationInput] {
        self.configuration
            .as_ref()
            .map_or(&[], |c| c.inputs.as_slice())
    }
    pub(crate) fn apply_configuration(
        &self,
        facts: &mut super::relations::FileFacts,
    ) -> EngineResult<()> {
        if let Some(configuration) = &self.configuration {
            configuration.apply(facts)?;
        }
        if matches!(
            facts.language,
            Language::Go | Language::Java | Language::C | Language::Cpp
        ) {
            facts.rules_fingerprint = super::code::compiled_fingerprint(
                facts,
                self.configuration.as_ref().map(|_| self.inputs()),
            )?;
        }
        Ok(())
    }
    pub(crate) fn matches_configuration(
        &self,
        facts: &super::relations::FileFacts,
    ) -> EngineResult<bool> {
        if !matches!(
            facts.language,
            Language::Go | Language::Java | Language::C | Language::Cpp
        ) {
            return Ok(true);
        }
        Ok(facts.rules_fingerprint
            == super::code::compiled_fingerprint(
                facts,
                self.configuration.as_ref().map(|_| self.inputs()),
            )?)
    }
    pub(crate) fn language(&self, path: &str, formats: &[FileFormat]) -> Option<Language> {
        if formats.contains(&FileFormat::C)
            && formats.contains(&FileFormat::Cpp)
            && let Some(root) = self
                .roots
                .iter()
                .filter(|r| Path::new(path).starts_with(&r.root))
                .max_by_key(|r| r.root.components().count())
            && matches!(root.language, Language::C | Language::Cpp)
        {
            return Some(root.language);
        }
        super::language(formats)
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
        let format = super::format(language);
        // Share the host catalog so Python .pyw/.pyi files accepted for text
        // indexing cannot fail solely because graph capture is enabled.
        let (stem, _) = file
            .rsplit_once('.')
            .filter(|(_, extension)| format.extensions().contains(extension))
            .ok_or_else(|| EngineError::invalid_argument("module extension mismatch"))?;
        let is_package = language == Language::Python && stem == "__init__";
        if !(is_package
            || language == Language::Rust && matches!(stem, "lib" | "main" | "mod")
            || language == Language::Go)
        {
            components.push(stem.to_owned());
        }
        if matches!(language, Language::C | Language::Cpp) {
            components.clear();
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

#[cfg(test)]
mod configuration_contract_tests {
    use super::*;
    use crate::api::index::graph::{GraphLanguage, GraphModuleRoot, GraphOptions};

    #[test]
    fn graph_module_configuration_agrees_at_both_entrypoints() {
        let workspace = tempfile::tempdir().expect("workspace");
        std::fs::create_dir(workspace.path().join("src")).expect("source directory");
        for (path, language, layout, namespace, valid) in [
            (
                "src",
                GraphLanguage::Rust,
                PackageLayout::ModRs,
                "crate",
                true,
            ),
            (
                "src",
                GraphLanguage::Python,
                PackageLayout::Src,
                "package",
                true,
            ),
            ("src", GraphLanguage::Go, PackageLayout::Flat, "go", true),
            ("src", GraphLanguage::Go, PackageLayout::Src, "go", false),
            (
                "src",
                GraphLanguage::Rust,
                PackageLayout::Src,
                "crate",
                false,
            ),
            (
                "src",
                GraphLanguage::Python,
                PackageLayout::ModRs,
                "package",
                false,
            ),
            (
                "../src",
                GraphLanguage::Rust,
                PackageLayout::Flat,
                "crate",
                false,
            ),
            ("src", GraphLanguage::Rust, PackageLayout::Flat, " ", false),
            (
                "missing",
                GraphLanguage::Rust,
                PackageLayout::Flat,
                "crate",
                false,
            ),
        ] {
            let options = GraphOptions {
                module_roots: vec![GraphModuleRoot {
                    root: path.into(),
                    language,
                    namespace: namespace.into(),
                    module_prefix: vec![],
                    layout,
                }],
                ..Default::default()
            };
            assert_eq!(options.normalized(workspace.path()).is_ok(), valid);
            let internal = ModuleRoots::new(vec![ModuleRoot {
                root: path.into(),
                language: language.fact_language(),
                namespace: namespace.into(),
                module_prefix: vec![],
                layout,
            }]);
            assert_eq!(
                internal
                    .and_then(|roots| roots.validate_workspace(workspace.path()))
                    .is_ok(),
                valid
            );
        }
    }
}
