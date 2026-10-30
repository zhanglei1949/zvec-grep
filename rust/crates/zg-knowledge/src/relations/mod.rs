//! Source-grounded, binding-independent facts. These are candidate facts, not
//! published graph records or a production in-memory graph backend.
pub mod bound;
mod code;
mod markdown;

pub use code::resolve_code;
pub use markdown::{extract_markdown, resolve_documents};

use serde::{Deserialize, Serialize};

use crate::{
    Error, Result,
    processing::{ProcessingContext, stable_digest},
};

pub const FACT_CONTRACT_VERSION: &str = "zg-relations-v1";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Language {
    Rust,
    Python,
    Markdown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Span {
    pub start: usize,
    pub end: usize,
}
impl Span {
    pub fn validate(self, text: &str) -> Result<()> {
        if self.start > self.end || text.get(self.start..self.end).is_none() {
            return Err(Error::InvalidInput("invalid decoded UTF-8 span"));
        }
        Ok(())
    }
    #[must_use]
    pub fn contains(self, other: Self) -> bool {
        self.start <= other.start && other.end <= self.end
    }
}

/// Positions refer to decoded UTF-8, never raw bytes. Both digests must be
/// revalidated by the host before serving evidence. G1 does not persist text.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceVersion {
    pub generation: String,
    pub path: String,
    pub raw_digest: String,
    pub decoded_digest: String,
}
impl SourceVersion {
    pub fn validate(&self, text: &str) -> Result<()> {
        validate_path(&self.path)?;
        if self.generation.is_empty()
            || !is_digest(&self.raw_digest)
            || self.decoded_digest != zg_storage::sha256_hex(text.as_bytes())
        {
            return Err(Error::InvalidInput("invalid source version"));
        }
        Ok(())
    }
}
fn is_digest(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|b| b.is_ascii_hexdigit())
}

pub(crate) fn validate_path(path: &str) -> Result<()> {
    if path.is_empty()
        || path.contains(['\\', '\0', ':'])
        || path.split('/').any(|p| matches!(p, "" | "." | ".."))
    {
        return Err(Error::InvalidInput(
            "expected normalized workspace-relative path",
        ));
    }
    Ok(())
}

/// The host supplies a source-root namespace. Equal module names in different
/// crates/packages must never resolve to each other by accident.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodeModule {
    pub namespace: String,
    pub path: Vec<String>,
    pub is_package: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeKind {
    File,
    Module,
    Symbol,
    Section,
}
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EntityKind {
    File,
    Module,
    Class,
    Interface,
    Function,
    Method,
    Field,
    Section,
    Type,
    Constant,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FactNode {
    pub id: String,
    pub kind: NodeKind,
    pub entity_kind: EntityKind,
    pub name: String,
    pub qualified_name: Vec<String>,
    pub parent: Option<String>,
    pub path_addressable: bool,
    /// Only a directly declared, unmodified function is a static call target.
    pub callable: bool,
    pub span: Span,
    /// Indices into the SAME extraction's host retrieval entities. G2 binds
    /// actual `EntityIds`; these are not persistent identity or evidence leases.
    pub entity_indices: Vec<usize>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScopeKind {
    Module,
    Function,
    Class,
    Block,
    Comprehension,
    Enum,
    /// Rust construction scopes, excluded from business traversal.
    InherentImpl,
    UnsupportedImpl,
    TypeAlias,
    Closure,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Scope {
    pub parent: Option<usize>,
    pub owner: String,
    pub kind: ScopeKind,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum BindingTarget {
    Node { id: String },
    Import { path: Vec<String> },
    GlobImport { path: Vec<String> },
    Redirect { scope: usize },
    Dynamic,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Binding {
    pub scope: usize,
    pub name: String,
    pub target: BindingTarget,
    /// Python locals shadow the entire function; Rust let bindings begin after
    /// their initializer. Multiple visible assignments remain conservative.
    pub visible_from: usize,
    /// Rust declaration visibility, retained for imports/re-exports. Empty is private.
    pub visibility: String,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RelationKind {
    Call,
    Import,
    DocumentLink,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Lookup {
    Name { parts: Vec<String> },
    Import { path: Vec<String> },
    Document { destination: String },
    Dynamic,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reference {
    pub id: String,
    pub owner: String,
    pub scope: usize,
    pub kind: RelationKind,
    pub span: Span,
    pub lookup: Lookup,
    pub entity_indices: Vec<usize>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FactDiagnostic {
    pub code: String,
    pub span: Option<Span>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Anchor {
    pub name: String,
    pub node_id: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileFacts {
    pub version: SourceVersion,
    pub language: Language,
    pub module: Option<CodeModule>,
    pub rules_fingerprint: String,
    pub complete: bool,
    pub nodes: Vec<FactNode>,
    pub scopes: Vec<Scope>,
    pub bindings: Vec<Binding>,
    pub references: Vec<Reference>,
    pub anchors: Vec<Anchor>,
    pub diagnostics: Vec<FactDiagnostic>,
}
impl FileFacts {
    pub fn new(
        version: SourceVersion,
        language: Language,
        module: Option<CodeModule>,
        text: &str,
    ) -> Result<Self> {
        version.validate(text)?;
        if (language == Language::Markdown) != module.is_none()
            || module
                .as_ref()
                .is_some_and(|m| m.namespace.is_empty() || m.path.iter().any(String::is_empty))
        {
            return Err(Error::InvalidInput(
                "code module namespace and path are required",
            ));
        }
        let id = node_id(&version.path, language, "file", &[], 0)?;
        Ok(Self {
            rules_fingerprint: stable_digest(&(FACT_CONTRACT_VERSION, language, &module))?,
            version,
            language,
            module,
            complete: true,
            nodes: vec![FactNode {
                id: id.clone(),
                kind: NodeKind::File,
                entity_kind: EntityKind::File,
                name: String::new(),
                qualified_name: Vec::new(),
                parent: None,
                path_addressable: false,
                callable: false,
                span: Span {
                    start: 0,
                    end: text.len(),
                },
                entity_indices: Vec::new(),
            }],
            scopes: vec![Scope {
                parent: None,
                owner: id,
                kind: ScopeKind::Module,
            }],
            bindings: Vec::new(),
            references: Vec::new(),
            anchors: Vec::new(),
            diagnostics: Vec::new(),
        })
    }
    fn redirect_ancestor(&self, from: usize, target: usize) -> bool {
        let mut parent = self.scopes.get(from).and_then(|s| s.parent);
        while let Some(index) = parent {
            if index == target {
                return true;
            }
            parent = self.scopes.get(index).and_then(|s| s.parent);
        }
        false
    }

    pub(crate) fn validate_structure(&self, context: &ProcessingContext) -> Result<()> {
        context.check()?;
        self.check_budget(context)?;
        validate_path(&self.version.path)?;
        if self.nodes.first().is_none_or(|n| n.kind != NodeKind::File)
            || self.scopes.is_empty()
            || self.scopes[0].parent.is_some()
            || (self.language == Language::Markdown) != self.module.is_none()
        {
            return Err(Error::Provider("invalid fact root"));
        }
        let mut ids = std::collections::HashSet::new();
        for (index, n) in self.nodes.iter().enumerate() {
            if (index == 0) != n.parent.is_none()
                || (n.kind == NodeKind::File) != (n.entity_kind == EntityKind::File)
                || n.parent.as_ref().is_some_and(|parent| {
                    self.nodes
                        .iter()
                        .find(|p| &p.id == parent)
                        .is_none_or(|p| !p.span.contains(n.span))
                })
            {
                return Err(Error::Provider("invalid business containment"));
            }
            if n.id.is_empty()
                || n.parent.as_ref().is_some_and(|p| !ids.contains(p))
                || !ids.insert(&n.id)
            {
                return Err(Error::Provider("invalid fact parent or identity"));
            }
        }
        for (i, s) in self.scopes.iter().enumerate() {
            if !ids.contains(&s.owner) || s.parent.is_some_and(|p| p >= i) {
                return Err(Error::Provider("invalid scope"));
            }
        }
        for (i, scope) in self.scopes.iter().enumerate() {
            if matches!(
                scope.kind,
                ScopeKind::InherentImpl | ScopeKind::UnsupportedImpl | ScopeKind::TypeAlias
            ) {
                let binding = self
                    .bindings
                    .iter()
                    .filter(|b| b.scope == i && b.name == "Self")
                    .collect::<Vec<_>>();
                if self.language != Language::Rust
                    || scope.parent.is_none()
                    || binding.len() != 1
                    || !matches!(
                        &binding[0].target,
                        BindingTarget::Dynamic | BindingTarget::Import { .. }
                    )
                    || matches!(&binding[0].target,BindingTarget::Import {path} if path.is_empty() || path.iter().any(String::is_empty))
                {
                    return Err(Error::Provider("invalid construction type context"));
                }
                if scope.kind == ScopeKind::TypeAlias
                    && self
                        .nodes
                        .iter()
                        .find(|n| n.id == scope.owner)
                        .is_none_or(|n| n.entity_kind != EntityKind::Type)
                {
                    return Err(Error::Provider("invalid type alias owner"));
                }
            }
        }
        for r in &self.references {
            if r.scope >= self.scopes.len() || !ids.contains(&r.owner) {
                return Err(Error::Provider("invalid reference"));
            }
        }
        for b in &self.bindings {
            if b.scope >= self.scopes.len()
                || matches!(&b.target, BindingTarget::Node {id} if !ids.contains(id))
                || matches!(&b.target, BindingTarget::Redirect {scope} if !self.redirect_ancestor(b.scope, *scope))
            {
                return Err(Error::Provider("invalid binding"));
            }
        }
        Ok(())
    }
    /// Validate source ranges and local references before a host consumes facts.
    pub fn validate(&self, text: &str, context: &ProcessingContext) -> Result<()> {
        self.version.validate(text)?;
        self.validate_structure(context)?;
        context.check_input(text.len())?;
        self.check_budget(context)?;
        let ids: std::collections::HashSet<_> = self.nodes.iter().map(|n| n.id.as_str()).collect();
        if ids.len() != self.nodes.len() || self.scopes.is_empty() || self.nodes.is_empty() {
            return Err(Error::Provider("invalid fact identity"));
        }
        for node in &self.nodes {
            node.span.validate(text)?;
            if node
                .parent
                .as_ref()
                .is_some_and(|p| !ids.contains(p.as_str()))
            {
                return Err(Error::Provider("unknown fact parent"));
            }
        }
        for (i, scope) in self.scopes.iter().enumerate() {
            if !ids.contains(scope.owner.as_str()) || scope.parent.is_some_and(|p| p >= i) {
                return Err(Error::Provider("invalid lexical scope"));
            }
        }
        let mut reference_ids = std::collections::HashSet::new();
        for r in &self.references {
            r.span.validate(text)?;
            if !reference_ids.insert(&r.id)
                || r.scope >= self.scopes.len()
                || !ids.contains(r.owner.as_str())
                || (self.language == Language::Markdown) != (r.kind == RelationKind::DocumentLink)
            {
                return Err(Error::Provider("invalid source reference"));
            }
        }
        for b in &self.bindings {
            if b.scope >= self.scopes.len()
                || b.visible_from > text.len()
                || b.name.is_empty()
                || matches!(&b.target, BindingTarget::Node {id} if !ids.contains(id.as_str()))
                || matches!(&b.target, BindingTarget::Redirect {scope} if !self.redirect_ancestor(b.scope, *scope))
            {
                return Err(Error::Provider("invalid lexical binding"));
            }
        }
        for a in &self.anchors {
            if !ids.contains(a.node_id.as_str()) {
                return Err(Error::Provider("invalid anchor target"));
            }
        }
        for d in &self.diagnostics {
            if let Some(s) = d.span {
                s.validate(text)?;
            }
        }
        Ok(())
    }
    pub fn check_budget(&self, context: &ProcessingContext) -> Result<()> {
        context.check_output(
            self.nodes
                .len()
                .saturating_add(self.scopes.len())
                .saturating_add(self.bindings.len())
                .saturating_add(self.references.len())
                .saturating_add(self.anchors.len())
                .saturating_add(self.diagnostics.len()),
        )
    }
    pub fn add_reference(
        &mut self,
        scope: usize,
        kind: RelationKind,
        span: Span,
        lookup: Lookup,
    ) -> Result<()> {
        let owner = self
            .scopes
            .get(scope)
            .ok_or(Error::Provider("invalid scope"))?
            .owner
            .clone();
        let id = stable_digest(&(&self.version, kind, span, self.references.len()))?;
        self.references.push(Reference {
            id,
            owner,
            scope,
            kind,
            span,
            lookup,
            entity_indices: Vec::new(),
        });
        Ok(())
    }
}

/// Identity excludes source digest, body and coordinates. Duplicate declarations
/// use a source-order discriminator; edits among duplicates may change identity.
pub fn node_id(
    path: &str,
    language: Language,
    kind: &str,
    qualified: &[String],
    discriminator: usize,
) -> Result<String> {
    stable_digest(&(
        FACT_CONTRACT_VERSION,
        path,
        language,
        kind,
        qualified,
        discriminator,
    ))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResolutionStatus {
    Resolved,
    Ambiguous,
    Unresolved,
    External,
    Unsupported,
    OutsideWorkspace,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Target {
    pub path: String,
    pub node_id: String,
    pub version: SourceVersion,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolvedReference {
    pub source: SourceVersion,
    pub reference: Reference,
    pub status: ResolutionStatus,
    pub reason: String,
    pub candidates: Vec<Target>,
    /// Conservative G1 read set: all input files in the same domain/namespace.
    /// Includes negative resolution dependencies. G2 must also guard the catalog
    /// revision against newly added files; per-file digests alone cannot do that.
    pub dependencies: Vec<SourceVersion>,
}

pub(crate) fn target(file: &FileFacts, node: &FactNode) -> Target {
    Target {
        path: file.version.path.clone(),
        node_id: node.id.clone(),
        version: file.version.clone(),
    }
}
pub(crate) fn resolved(
    source: &FileFacts,
    r: &Reference,
    candidates: Vec<Target>,
    missing: ResolutionStatus,
    reason: &str,
    dependencies: Vec<SourceVersion>,
) -> ResolvedReference {
    let status = match candidates.len() {
        0 => missing,
        1 => ResolutionStatus::Resolved,
        _ => ResolutionStatus::Ambiguous,
    };
    ResolvedReference {
        source: source.version.clone(),
        reference: r.clone(),
        status,
        reason: if status == ResolutionStatus::Ambiguous {
            "multiple_targets".into()
        } else {
            reason.into()
        },
        candidates,
        dependencies,
    }
}
