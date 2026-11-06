//! Host-bound candidate facts. No source bodies, vectors, ASTs or extraction
//! indices are persisted. The host still owns text eligibility and publication.
use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use super::{FileFacts, Language, ResolutionStatus, SourceVersion, Target};
use crate::{
    Error, Result,
    processing::{ProcessingContext, stable_digest},
};

pub const FACTS_FORMAT_VERSION: u64 = 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IndexStatus {
    NotIndexed,
    Indexed,
    Deleting,
    Failed,
}

/// A row in the complete host inventory, including unsupported/failed files.
/// Failed messages are intentionally omitted. No timestamp implies eligibility.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileRecord {
    pub path: String,
    pub file_id: u32,
    pub index_status: IndexStatus,
    pub content_hash: Option<String>,
    pub size_bytes: u64,
    pub text_revision: u64,
    pub storage_generation: String,
    pub entity_count: u64,
    pub entity_ids_digest: String,
}

pub fn entity_ids_digest(ids: &[String]) -> Result<String> {
    let mut sorted = ids.to_vec();
    sorted.sort();
    if sorted.windows(2).any(|w| w[0] == w[1]) {
        return Err(Error::InvalidInput("duplicate host entity ID"));
    }
    stable_digest(&sorted)
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BoundFileFacts {
    pub record: FileRecord,
    pub facts: Option<FileFacts>,
    /// Parallel to nodes/references in source order; values are actual `EntityIds`.
    pub node_entities: Vec<Vec<String>>,
    pub reference_entities: Vec<Vec<String>>,
    #[serde(default)]
    pub text_map: Vec<crate::query::TextMap>,
    #[serde(default)]
    pub text_binding_digest: Option<String>,
}
impl BoundFileFacts {
    /// Consume indices from THIS extraction and bind the IDs allocated by the
    /// host. A later extraction cannot be substituted, even with equal counts.
    pub fn bind(
        mut facts: FileFacts,
        mut record: FileRecord,
        entity_ids: &[String],
        context: &ProcessingContext,
    ) -> Result<Self> {
        record.entity_ids_digest = entity_ids_digest(entity_ids)?;
        record.entity_count =
            u64::try_from(entity_ids.len()).map_err(|_| Error::ResourceExhausted)?;
        for id in entity_ids {
            validate_entity_id(id, record.file_id)?;
        }
        let bind = |indices: &mut Vec<usize>| -> Result<Vec<String>> {
            let bound = indices
                .iter()
                .map(|i| {
                    entity_ids
                        .get(*i)
                        .cloned()
                        .ok_or(Error::Provider("entity binding out of range"))
                })
                .collect();
            indices.clear();
            bound
        };
        let node_entities = facts
            .nodes
            .iter_mut()
            .map(|n| bind(&mut n.entity_indices))
            .collect::<Result<_>>()?;
        let reference_entities = facts
            .references
            .iter_mut()
            .map(|r| bind(&mut r.entity_indices))
            .collect::<Result<_>>()?;
        let output = Self {
            record,
            facts: Some(facts),
            node_entities,
            reference_entities,
            text_map: Vec::new(),
            text_binding_digest: None,
        };
        output.validate(context)?;
        Ok(output)
    }

    pub fn inventory_only(record: FileRecord, context: &ProcessingContext) -> Result<Self> {
        let output = Self {
            record,
            facts: None,
            node_entities: Vec::new(),
            reference_entities: Vec::new(),
            text_map: Vec::new(),
            text_binding_digest: None,
        };
        output.validate(context)?;
        Ok(output)
    }

    pub fn validate(&self, context: &ProcessingContext) -> Result<()> {
        context.check()?;
        for map in &self.text_map {
            map.validate()
                .map_err(|_| Error::InvalidInput("invalid TextMap"))?;
            if map.source_id != self.record.path {
                return Err(Error::InvalidInput("TextMap source mismatch"));
            }
            validate_entity_id(&map.entity_id, self.record.file_id)?;
        }
        if self
            .text_binding_digest
            .as_ref()
            .is_some_and(|d| !super::is_digest(d))
        {
            return Err(Error::InvalidInput("invalid text binding digest"));
        }
        let r = &self.record;
        super::validate_path(&r.path)?;
        if r.storage_generation.is_empty()
            || !super::is_digest(&r.entity_ids_digest)
            || r.content_hash
                .as_ref()
                .is_some_and(|s| !super::is_digest(s))
            || (r.index_status == IndexStatus::Indexed && r.content_hash.is_none())
        {
            return Err(Error::InvalidInput("invalid host file version"));
        }
        let Some(f) = &self.facts else {
            if !self.node_entities.is_empty() || !self.reference_entities.is_empty() {
                return Err(Error::InvalidInput("bindings without facts"));
            }
            return Ok(());
        };
        f.validate_structure(context)?;
        if f.version.path != r.path
            || f.version.generation != r.storage_generation
            || Some(&f.version.raw_digest) != r.content_hash.as_ref()
            || !super::is_digest(&f.version.decoded_digest)
            || !super::is_digest(&f.rules_fingerprint)
            || self.node_entities.len() != f.nodes.len()
            || self.reference_entities.len() != f.references.len()
            || f.nodes.iter().any(|n| !n.entity_indices.is_empty())
            || f.references.iter().any(|r| !r.entity_indices.is_empty())
        {
            return Err(Error::InvalidInput("fact and host version mismatch"));
        }
        let root = f.nodes[0].span;
        if root.start != 0
            || f.module
                .as_ref()
                .is_some_and(|m| m.namespace.is_empty() || m.path.iter().any(String::is_empty))
        {
            return Err(Error::Provider("invalid persisted file root or module"));
        }
        let mut refs = BTreeSet::new();
        for n in &f.nodes {
            if n.span.start > n.span.end || !root.contains(n.span) {
                return Err(Error::Provider("invalid persisted span"));
            }
        }
        for reference in &f.references {
            if !refs.insert(&reference.id)
                || reference.span.start > reference.span.end
                || !root.contains(reference.span)
                || (f.language == Language::Markdown)
                    != (reference.kind == super::RelationKind::DocumentLink)
            {
                return Err(Error::Provider("invalid persisted reference"));
            }
        }
        if f.bindings
            .iter()
            .any(|b| b.name.is_empty() || b.visible_from > root.end)
            || f.anchors
                .iter()
                .any(|a| !f.nodes.iter().any(|n| n.id == a.node_id))
            || f.diagnostics.iter().any(|d| {
                d.code.is_empty() || d.span.is_some_and(|s| s.start > s.end || !root.contains(s))
            })
        {
            return Err(Error::Provider("invalid persisted structure"));
        }
        for ids in self.node_entities.iter().chain(&self.reference_entities) {
            let mut seen = BTreeSet::new();
            for id in ids {
                validate_entity_id(id, r.file_id)?;
                if !seen.insert(id) {
                    return Err(Error::InvalidInput("duplicate evidence binding"));
                }
            }
        }
        Ok(())
    }
}

fn validate_entity_id(id: &str, file_id: u32) -> Result<()> {
    if id.len() != 32
        || !id.bytes().all(|b| b.is_ascii_hexdigit())
        || !id.starts_with(&format!("{file_id:08x}"))
    {
        return Err(Error::InvalidInput("invalid host entity ID"));
    }
    Ok(())
}

/// Build-local ordinal identity, NOT a cross-revision business identity.
pub fn synthetic_id(path: &str, kind: &str, ordinal: usize) -> Result<String> {
    super::validate_path(path)?;
    if !matches!(kind, "scope" | "binding" | "diagnostic" | "anchor") {
        return Err(Error::InvalidInput("unknown ordinal identity kind"));
    }
    // Ordinal rows keep their v7 identity when the raw fact enum gains variants.
    stable_digest(&("zg-relations-v2", path, kind, ordinal))
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct CatalogKey {
    pub domain: String,
    pub language: String,
    pub namespace: String,
}
impl CatalogKey {
    fn for_facts(f: &FileFacts) -> Self {
        Self {
            domain: if f.language == Language::Markdown {
                "documents"
            } else {
                "code"
            }
            .into(),
            language: match f.language {
                Language::Rust => "rust",
                Language::Python => "python",
                Language::Go => "go",
                Language::Java => "java",
                Language::C => "c",
                Language::Cpp => "cpp",
                Language::Markdown => "markdown",
            }
            .into(),
            namespace: f
                .module
                .as_ref()
                .map_or_else(String::new, |m| m.namespace.clone()),
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CatalogRevision {
    pub revision: String,
    pub key: CatalogKey,
    pub members: Vec<String>,
    pub members_digest: String,
    pub config_fingerprint: String,
    pub selection_fingerprint: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TargetVersion {
    pub node_id: String,
    pub source: SourceVersion,
    pub text_revision: u64,
    pub entity_ids_digest: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredResolution {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub relation_source: Option<TargetVersion>,
    pub file_path: String,
    pub reference_id: String,
    pub status: ResolutionStatus,
    pub reason: String,
    pub targets: Vec<TargetVersion>,
    pub catalog_revision: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FactCatalog {
    pub files: Vec<BoundFileFacts>,
    pub catalogs: Vec<CatalogRevision>,
    pub resolutions: Vec<StoredResolution>,
    pub config_fingerprint: String,
    pub selection_fingerprint: String,
    pub semantic: crate::semantic::SemanticCatalog,
}
impl FactCatalog {
    /// Reuses the G1 resolver; the stored read set is one revision per catalog,
    /// not reference × file edges. All host inventory participates in the
    /// selection digest, including unsupported targets and negative lookups.
    pub fn resolve(
        mut files: Vec<BoundFileFacts>,
        config_fingerprint: String,
        selection_fingerprint: String,
        context: &ProcessingContext,
    ) -> Result<Self> {
        files.sort_by(|a, b| a.record.path.cmp(&b.record.path));
        let (catalogs, resolutions) =
            Self::derive(&files, &config_fingerprint, &selection_fingerprint, context)?;
        Ok(Self {
            files,
            catalogs,
            resolutions,
            config_fingerprint,
            selection_fingerprint,
            semantic: crate::semantic::SemanticCatalog::default(),
        })
    }

    #[expect(
        clippy::too_many_lines,
        reason = "validate the whole inventory and compact the shared resolver read set in one place"
    )]
    fn derive(
        files: &[BoundFileFacts],
        config_fingerprint: &str,
        selection_fingerprint: &str,
        context: &ProcessingContext,
    ) -> Result<(Vec<CatalogRevision>, Vec<StoredResolution>)> {
        if !super::is_digest(config_fingerprint) || !super::is_digest(selection_fingerprint) {
            return Err(Error::InvalidInput("invalid catalog fingerprint"));
        }
        let mut paths = BTreeSet::new();
        let mut ids = BTreeSet::new();
        let mut groups: BTreeMap<CatalogKey, Vec<&BoundFileFacts>> = BTreeMap::new();
        let mut node_ids = BTreeSet::new();
        let mut reference_ids = BTreeSet::new();
        for f in files {
            f.validate(context)?;
            if !paths.insert(&f.record.path) || !ids.insert(f.record.file_id) {
                return Err(Error::InvalidInput("duplicate host file identity"));
            }
            if let Some(facts) = &f.facts {
                if facts.nodes.iter().any(|n| !node_ids.insert(&n.id))
                    || facts
                        .references
                        .iter()
                        .any(|r| !reference_ids.insert(&r.id))
                {
                    return Err(Error::InvalidInput("duplicate catalog fact identity"));
                }
                groups
                    .entry(CatalogKey::for_facts(facts))
                    .or_default()
                    .push(f);
            }
        }
        context.check_output(files.len())?;
        let inventory_digest = stable_digest(&files.iter().map(|f| &f.record).collect::<Vec<_>>())?;
        let catalogs = groups
            .iter()
            .map(|(key, members)| {
                let members: Vec<_> = if key.domain == "documents" {
                    files.iter().collect()
                } else {
                    members.clone()
                };
                let members_digest = stable_digest(
                    &members
                        .iter()
                        .map(|f| {
                            (
                                &f.record,
                                f.facts
                                    .as_ref()
                                    .map(|v| (&v.version, &v.rules_fingerprint, v.complete)),
                            )
                        })
                        .collect::<Vec<_>>(),
                )?;
                let revision = stable_digest(&(
                    FACTS_FORMAT_VERSION,
                    key,
                    &members_digest,
                    &inventory_digest,
                    &config_fingerprint,
                    &selection_fingerprint,
                ))?;
                Ok(CatalogRevision {
                    revision,
                    key: key.clone(),
                    members: members.iter().map(|f| f.record.path.clone()).collect(),
                    members_digest,
                    config_fingerprint: config_fingerprint.to_string(),
                    selection_fingerprint: selection_fingerprint.to_string(),
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let eligible: Vec<&FileFacts> = files
            .iter()
            .filter(|f| f.record.index_status == IndexStatus::Indexed)
            .filter_map(|f| f.facts.as_ref())
            .collect();
        let indexed_paths: Vec<_> = files
            .iter()
            .filter(|f| f.record.index_status == IndexStatus::Indexed)
            .map(|f| f.record.path.clone())
            .collect();
        let file_map: BTreeMap<_, _> = files.iter().map(|f| (f.record.path.as_str(), f)).collect();
        let mut resolutions = Vec::new();
        let mut collect = |r: super::ResolvedReference, _: &[&FileFacts]| -> Result<()> {
            let source = file_map[r.source.path.as_str()];
            let key = CatalogKey::for_facts(
                source
                    .facts
                    .as_ref()
                    .ok_or(Error::Provider("missing source facts"))?,
            );
            let catalog_revision = catalogs
                .iter()
                .find(|c| c.key == key)
                .ok_or(Error::Provider("missing catalog"))?
                .revision
                .clone();
            let targets = r
                .candidates
                .iter()
                .map(|t: &Target| {
                    let f = file_map
                        .get(t.path.as_str())
                        .ok_or(Error::Provider("missing target file"))?;
                    if f.facts.as_ref().is_none_or(|facts| {
                        facts.version != t.version || !facts.nodes.iter().any(|n| n.id == t.node_id)
                    }) {
                        return Err(Error::Provider("invalid target version"));
                    }
                    Ok(TargetVersion {
                        node_id: t.node_id.clone(),
                        source: t.version.clone(),
                        text_revision: f.record.text_revision,
                        entity_ids_digest: f.record.entity_ids_digest.clone(),
                    })
                })
                .collect::<Result<Vec<_>>>()?;
            resolutions.push(StoredResolution {
                relation_source: r
                    .relation_source
                    .map(|target| {
                        let file = file_map
                            .get(target.path.as_str())
                            .ok_or(Error::Provider("missing relation source file"))?;
                        Ok(TargetVersion {
                            node_id: target.node_id,
                            source: target.version,
                            text_revision: file.record.text_revision,
                            entity_ids_digest: file.record.entity_ids_digest.clone(),
                        })
                    })
                    .transpose()?,
                file_path: r.source.path,
                reference_id: r.reference.id,
                status: r.status,
                reason: r.reason,
                targets,
                catalog_revision,
            });
            Ok(())
        };
        super::code::visit_code(&eligible, context, &mut collect)?;
        super::markdown::visit_documents(&eligible, &indexed_paths, context, &mut collect)?;
        resolutions
            .sort_by(|a, b| (&a.file_path, &a.reference_id).cmp(&(&b.file_path, &b.reference_id)));
        context.check_output(resolutions.len().saturating_add(files.len()))?;
        Ok((catalogs, resolutions))
    }

    pub fn with_semantic(
        mut self,
        mut semantic: crate::semantic::SemanticCatalog,
        context: &ProcessingContext,
    ) -> Result<Self> {
        semantic.normalize()?;
        self.semantic = semantic;
        self.validate_semantic(context)?;
        Ok(self)
    }

    /// Keep the validated base directory while rebuilding source structure.
    /// A changed binding retains its evidence as an auxiliary, never a live edge.
    pub fn inherit_semantic(
        mut self,
        previous: &Self,
        observed_sources: &[crate::semantic::SourceChange],
        context: &ProcessingContext,
    ) -> Result<Self> {
        previous.validate_semantic(context)?;
        self.semantic = previous.semantic.clone();
        let mut observed = BTreeSet::new();
        for change in observed_sources {
            context.check()?;
            change.source().validate()?;
            if !observed.insert(&change.source().source_id) {
                return Err(Error::InvalidInput("duplicate observed source"));
            }
            if matches!(
                change,
                crate::semantic::SourceChange::Upsert {
                    semantic: Some(_),
                    ..
                }
            ) {
                return Err(Error::InvalidInput(
                    "complete semantic input must use apply_sources",
                ));
            }
            self.semantic.change_source(change)?;
        }
        if !self.semantic.mentions.is_empty() {
            let (nodes, references) = Self::semantic_bindings(&self.files);
            for mention in std::mem::take(&mut self.semantic.mentions) {
                context.check()?;
                if mention_bound(&mention, &nodes, &references) {
                    self.semantic.mentions.push(mention);
                } else {
                    self.semantic.deferred_mentions.push(mention);
                }
            }
        }
        self.semantic.normalize()?;
        self.validate_semantic(context)?;
        Ok(self)
    }

    pub fn apply_sources(
        self,
        input: &crate::semantic::SemanticInput,
        context: &ProcessingContext,
    ) -> Result<Self> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|_| Error::InvalidInput("system clock before Unix epoch"))?
            .as_millis();
        self.apply_sources_at(
            input,
            context,
            u64::try_from(now).map_err(|_| Error::ResourceExhausted)?,
        )
    }

    pub fn apply_sources_at(
        mut self,
        input: &crate::semantic::SemanticInput,
        context: &ProcessingContext,
        operation_ms: u64,
    ) -> Result<Self> {
        let files: BTreeMap<_, _> = self
            .files
            .iter()
            .map(|file| (file.record.path.as_str(), file))
            .collect();
        for evidence in input
            .fact_changes
            .iter()
            .filter(|change| {
                !self.semantic.changes.iter().any(|record| {
                    matches!(record, crate::semantic::SemanticChange::Fact { change: saved } if saved == *change)
                })
            })
            .map(crate::semantic::FactChange::evidence)
            .chain(input.episode_withdrawals.iter().filter_map(|change| {
                (!self.semantic.changes.iter().any(|record| {
                    matches!(record, crate::semantic::SemanticChange::Episode { change: saved } if saved == change)
                }))
                .then_some(&change.evidence)
            }))
        {
            context.check()?;
            if let Some(file) = files.get(evidence.source.source_id.as_str()) {
                if file.record.index_status != IndexStatus::Indexed
                    || file.record.content_hash.as_ref()
                        != Some(&evidence.source.source_version)
                {
                    return Err(Error::Conflict("correction file version conflicts"));
                }
                if let Some(locator) = &evidence.locator {
                    let facts = file.facts.as_ref().ok_or(Error::InvalidInput(
                        "correction source positions are unavailable",
                    ))?;
                    if locator.raw_digest != facts.version.raw_digest
                        || locator.decoded_digest != facts.version.decoded_digest
                        || facts.nodes.first().is_none_or(|node| {
                            locator.span.start < node.span.start as u64
                                || locator.span.end > node.span.end as u64
                        })
                    {
                        return Err(Error::Conflict("correction source binding mismatch"));
                    }
                }
            }
        }
        self.semantic = self
            .semantic
            .apply_sources_at(input, context, operation_ms)?;
        self.validate_semantic(context)?;
        Ok(self)
    }

    pub(crate) fn validate_semantic(&self, context: &ProcessingContext) -> Result<()> {
        self.semantic.validate(context)?;
        if self.semantic.mentions.is_empty() {
            return Ok(());
        }
        let (nodes, references) = Self::semantic_bindings(&self.files);
        for mention in &self.semantic.mentions {
            context.check()?;
            if !mention_bound(mention, &nodes, &references) {
                return Err(Error::InvalidInput("mention source binding mismatch"));
            }
        }
        Ok(())
    }

    fn semantic_bindings(
        files: &[BoundFileFacts],
    ) -> (
        BTreeMap<&str, (&FileFacts, &super::FactNode)>,
        BTreeSet<&str>,
    ) {
        let files: BTreeMap<_, _> = files
            .iter()
            .filter_map(|file| {
                file.facts
                    .as_ref()
                    .map(|facts| (file.record.path.as_str(), facts))
            })
            .collect();
        let nodes: BTreeMap<_, _> = files
            .values()
            .flat_map(|facts| {
                facts
                    .nodes
                    .iter()
                    .map(move |node| (node.id.as_str(), (*facts, node)))
            })
            .collect();
        let references: BTreeSet<_> = files
            .values()
            .flat_map(|f| f.references.iter().map(|r| r.id.as_str()))
            .collect();
        (nodes, references)
    }

    pub fn validate(&self, context: &ProcessingContext) -> Result<()> {
        if !self
            .files
            .is_sorted_by(|a, b| a.record.path <= b.record.path)
        {
            return Err(Error::Provider("inconsistent candidate catalog"));
        }
        let (catalogs, resolutions) = Self::derive(
            &self.files,
            &self.config_fingerprint,
            &self.selection_fingerprint,
            context,
        )?;
        self.validate_semantic(context)?;
        let mut semantic = self.semantic.clone();
        semantic.normalize()?;
        if semantic != self.semantic || catalogs != self.catalogs || resolutions != self.resolutions
        {
            return Err(Error::Provider("inconsistent candidate catalog"));
        }
        Ok(())
    }
}

fn mention_bound(
    mention: &crate::semantic::Mention,
    nodes: &BTreeMap<&str, (&FileFacts, &super::FactNode)>,
    references: &BTreeSet<&str>,
) -> bool {
    nodes
        .get(mention.owner_id.as_str())
        .is_some_and(|(facts, owner)| {
            !references.contains(mention.ref_id.as_str())
                && mention.evidence.source_id == facts.version.path
                && mention.evidence.raw_digest == facts.version.raw_digest
                && mention.evidence.decoded_digest == facts.version.decoded_digest
                && mention.evidence.span.start >= owner.span.start as u64
                && mention.evidence.span.end <= owner.span.end as u64
        })
}
