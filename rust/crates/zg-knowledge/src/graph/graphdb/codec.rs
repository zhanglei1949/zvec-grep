//! Typed fact rows and versioned JSON attributes; no AST/body storage.
mod semantic;
use super::super::{
    GraphError, GraphErrorKind, GraphResult, profile,
    schema::{
        AnchorRow, AssociationRow, BindingRow, CatalogRow, DeferredMentionRow, DiagnosticRow,
        EpisodeRow, FileRow, MetadataRow, NodeRow, RELATIONS, ReferenceRow, STRUCTURE_RELATIONS,
        ScalarFactRow, ScopeRow, SemanticChangeRow, SemanticEntityRow, SemanticOperationRow,
        SemanticRelationRow, SemanticSourceRow,
    },
};
use super::StoreCounts;
use crate::{
    processing::{ProcessingContext, canonical_json},
    relations::{
        Anchor, Binding, BindingTarget, CodeModule, FactDiagnostic, FactNode, FileFacts, Reference,
        ResolutionStatus, Scope, SourceVersion, Span,
        bound::{BoundFileFacts, FACTS_FORMAT_VERSION, FactCatalog, FileRecord, synthetic_id},
    },
};
use ::graphdb::{
    Batch, Column, EndpointMapping, ImportOptions, LogicalType, QueryOptions, Schema, Transaction,
    params,
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::{collections::BTreeMap, sync::Arc};

fn invalid() -> GraphError {
    GraphError::local(GraphErrorKind::IncompatibleFormat)
}

// Stream the v7 canonical field order while materializing only one source or
// resolution as a JSON Value at a time, rather than a second complete catalog.
pub(super) fn catalog_digest(catalog: &FactCatalog) -> GraphResult<String> {
    let FactCatalog {
        files,
        catalogs,
        resolutions,
        config_fingerprint,
        selection_fingerprint,
        semantic,
    } = catalog;
    let mut output = String::from("{\"catalogs\":[");
    macro_rules! values {
        ($items:expr) => {
            for (index, item) in $items.iter().enumerate() {
                if index != 0 {
                    output.push(',');
                }
                output.push_str(&canonical_json(item)?);
            }
        };
    }
    values!(catalogs);
    output.push_str("],\"config_fingerprint\":");
    output.push_str(&canonical_json(config_fingerprint)?);
    output.push_str(",\"files\":[");
    values!(files);
    output.push_str("],\"resolutions\":[");
    values!(resolutions);
    output.push_str("],\"selection_fingerprint\":");
    output.push_str(&canonical_json(selection_fingerprint)?);
    output.push_str(",\"semantic\":{\"associations\":[");
    values!(&semantic.associations);
    output.push_str("],\"changes\":[");
    values!(&semantic.changes);
    output.push_str("],\"deferred_mentions\":[");
    values!(&semantic.deferred_mentions);
    output.push_str("],\"entities\":[");
    values!(&semantic.entities);
    output.push_str("],\"episodes\":[");
    values!(&semantic.episodes);
    output.push_str("],\"facts\":[");
    values!(&semantic.facts);
    output.push_str("],\"last_operation\":");
    output.push_str(&canonical_json(&semantic.last_operation)?);
    output.push_str(",\"mentions\":[");
    values!(&semantic.mentions);
    output.push_str("],\"sources\":[");
    values!(&semantic.sources);
    output.push_str("]}}");
    Ok(zg_storage::sha256_hex(output.as_bytes()))
}
fn number(value: usize) -> GraphResult<u64> {
    u64::try_from(value).map_err(|_| invalid())
}
fn ordinal(value: u64) -> GraphResult<usize> {
    usize::try_from(value).map_err(|_| invalid())
}
fn tag<T: Serialize>(value: &T) -> GraphResult<String> {
    serde_json::to_value(value)
        .map_err(|_| invalid())?
        .as_str()
        .map(str::to_owned)
        .ok_or_else(invalid)
}
pub(super) fn untag<T: DeserializeOwned>(value: &str) -> GraphResult<T> {
    serde_json::from_value(serde_json::Value::String(value.into())).map_err(|_| invalid())
}
fn json<T: Serialize>(value: &T) -> GraphResult<String> {
    Ok(canonical_json(
        &serde_json::json!({"version":1,"value":value}),
    )?)
}
pub(super) fn unjson<T: DeserializeOwned>(value: &str) -> GraphResult<T> {
    let mut object: serde_json::Value = serde_json::from_str(value).map_err(|_| invalid())?;
    if object["version"] != 1 || object.as_object().is_none_or(|o| o.len() != 2) {
        return Err(invalid());
    }
    serde_json::from_value(object["value"].take()).map_err(|_| invalid())
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub(super) struct Edge {
    name: String,
    source: String,
    target: String,
    version: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(in crate::graph) struct ResolvedReference {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub relation_source: Option<crate::relations::bound::TargetVersion>,
    pub reference: ReferenceRow,
    pub target: crate::relations::bound::TargetVersion,
    pub catalog_revision: String,
}

/// Borrowed rows in endpoint-before-edge order. A batch never copies the full
/// catalog and byte accounting includes tags, field names and JSON escaping.
#[derive(Serialize)]
pub(super) enum Mutation<'a> {
    File(&'a FileRow),
    Node(&'a NodeRow),
    Reference(&'a ReferenceRow),
    Anchor(&'a AnchorRow),
    Catalog(&'a CatalogRow),
    Scope(&'a ScopeRow),
    Binding(&'a BindingRow),
    Diagnostic(&'a DiagnosticRow),
    SemanticSource(&'a SemanticSourceRow),
    SemanticEntity(&'a SemanticEntityRow),
    Episode(&'a EpisodeRow),
    ScalarFact(&'a ScalarFactRow),
    Association(&'a AssociationRow),
    DeferredMention(&'a DeferredMentionRow),
    SemanticChange(&'a SemanticChangeRow),
    SemanticOperation(&'a SemanticOperationRow),
    SemanticRelation(&'a SemanticRelationRow),
    Edge(&'a Edge),
}
fn chained_digest(
    previous: &str,
    scratch: &mut Vec<u8>,
    tag: &str,
    write: impl FnOnce(&mut Vec<u8>) -> GraphResult<()>,
) -> GraphResult<String> {
    scratch.clear();
    scratch.push(b'[');
    serde_json::to_writer(&mut *scratch, previous).map_err(|_| invalid())?;
    scratch.extend_from_slice(b",{");
    serde_json::to_writer(&mut *scratch, tag).map_err(|_| invalid())?;
    scratch.push(b':');
    write(scratch)?;
    scratch.extend_from_slice(b"}]");
    Ok(zg_storage::sha256_hex(scratch))
}

impl Mutation<'_> {
    /// The v7 row digest chain, checked against the typed canonical oracle.
    /// Only the scratch allocation is reused; this is not a new hash format.
    #[cfg(test)]
    pub(super) fn update_digest(
        &self,
        previous: &str,
        scratch: &mut Vec<u8>,
    ) -> GraphResult<String> {
        scratch.clear();
        scratch.push(b'[');
        serde_json::to_writer(&mut *scratch, previous).map_err(|_| invalid())?;
        scratch.extend_from_slice(b",{");
        macro_rules! row {
            ($tag:literal, $value:expr) => {{
                scratch.extend_from_slice(concat!("\"", $tag, "\":").as_bytes());
                $value.write_canonical(scratch)?;
            }};
        }
        match self {
            Self::File(value) => row!("File", value),
            Self::Node(value) => row!("Node", value),
            Self::Reference(value) => row!("Reference", value),
            Self::Anchor(value) => row!("Anchor", value),
            Self::Catalog(value) => row!("Catalog", value),
            Self::Scope(value) => row!("Scope", value),
            Self::Binding(value) => row!("Binding", value),
            Self::Diagnostic(value) => row!("Diagnostic", value),
            Self::SemanticSource(value) => row!("SemanticSource", value),
            Self::SemanticEntity(value) => row!("SemanticEntity", value),
            Self::Episode(value) => row!("Episode", value),
            Self::ScalarFact(value) => row!("ScalarFact", value),
            Self::Association(value) => row!("Association", value),
            Self::SemanticChange(value) => row!("SemanticChange", value),
            Self::SemanticOperation(value) => row!("SemanticOperation", value),
            Self::DeferredMention(value) => row!("DeferredMention", value),
            Self::SemanticRelation(value) => row!("SemanticRelation", value),
            Self::Edge(value) => {
                scratch.extend_from_slice(b"\"Edge\":");
                // Edge's declaration already follows canonical key order.
                serde_json::to_writer(&mut *scratch, value).map_err(|_| invalid())?;
            }
        }
        scratch.extend_from_slice(b"}]");
        Ok(zg_storage::sha256_hex(scratch))
    }
}
fn edge_reference_id(name: &str, payload: &str) -> GraphResult<String> {
    if name == "KgMentions" {
        Ok(unjson::<crate::semantic::Mention>(payload)?.ref_id)
    } else {
        Ok(unjson::<ResolvedReference>(payload)?.reference.id)
    }
}
impl Edge {
    fn import(
        edges: &[&Self],
        name: &str,
        source: &str,
        target: &str,
        versioned: bool,
        tx: &mut Transaction<'_>,
        options: &QueryOptions,
    ) -> GraphResult<()> {
        if edges.is_empty() {
            return Ok(());
        }
        if !versioned && edges.iter().any(|edge| edge.version.is_some()) {
            return Err(invalid());
        }
        let mut fields = vec![
            ("source".into(), LogicalType::String),
            ("target".into(), LogicalType::String),
        ];
        if versioned {
            fields.extend([
                ("ref_id".into(), LogicalType::String),
                ("data".into(), LogicalType::String),
            ]);
        }
        let schema = Arc::new(Schema::try_new(fields)?);
        for chunk in edges.chunks(65_536) {
            let mut columns = vec![
                Column::strings(chunk.iter().map(|edge| Some(edge.source.clone())).collect()),
                Column::strings(chunk.iter().map(|edge| Some(edge.target.clone())).collect()),
            ];
            if versioned {
                let resolved = chunk
                    .iter()
                    .map(|edge| {
                        edge.version
                            .as_ref()
                            .map(|version| edge_reference_id(name, version))
                            .transpose()
                    })
                    .collect::<GraphResult<Vec<_>>>()?;
                columns.push(Column::strings(resolved));
                columns.push(Column::strings(
                    chunk.iter().map(|edge| edge.version.clone()).collect(),
                ));
            }
            let batch = Batch::try_new(schema.clone(), chunk.len(), columns)?;
            let summary = tx.import_batches(
                name,
                batch.into_source(),
                ImportOptions {
                    endpoints: Some(EndpointMapping {
                        source_label: source.into(),
                        source_column: "source".into(),
                        target_label: target.into(),
                        target_column: "target".into(),
                    }),
                    query: options.clone(),
                    ..ImportOptions::default()
                },
            )?;
            if summary.imported_rows != chunk.len() as u64 {
                return Err(GraphError::local(GraphErrorKind::Storage));
            }
        }
        Ok(())
    }

    fn write(&self, tx: &mut Transaction<'_>, options: &QueryOptions) -> GraphResult<()> {
        let (_, source, target, _) = RELATIONS
            .iter()
            .find(|r| r.0 == self.name)
            .ok_or_else(invalid)?;
        let key = |name| match name {
            "KgFile" => "path",
            "KgCatalogRevision" => "revision",
            _ => "id",
        };
        let property = if self.version.is_some() {
            " {ref_id: $ref_id, data: $version}"
        } else {
            ""
        };
        let query = format!(
            "MATCH (a:{source} {{{}: $source}}), (b:{target} {{{}: $target}}) CREATE (a)-[:{}{property}]->(b)",
            key(*source),
            key(*target),
            self.name
        );
        if let Some(version) = &self.version {
            tx.execute_with(
                &query,
                params! {"source"=>&self.source,"target"=>&self.target,"version"=>version,"ref_id"=>&edge_reference_id(&self.name, version)?},
                options,
            )?;
        } else {
            tx.execute_with(
                &query,
                params! {"source"=>&self.source,"target"=>&self.target},
                options,
            )?;
        }
        Ok(())
    }
}
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct FactRows {
    files: Vec<FileRow>,
    nodes: Vec<NodeRow>,
    scopes: Vec<ScopeRow>,
    bindings: Vec<BindingRow>,
    references: Vec<ReferenceRow>,
    anchors: Vec<AnchorRow>,
    diagnostics: Vec<DiagnosticRow>,
    catalogs: Vec<CatalogRow>,
    semantic_sources: Vec<SemanticSourceRow>,
    semantic_entities: Vec<SemanticEntityRow>,
    episodes: Vec<EpisodeRow>,
    scalar_facts: Vec<ScalarFactRow>,
    associations: Vec<AssociationRow>,
    semantic_relations: Vec<SemanticRelationRow>,
    deferred_mentions: Vec<DeferredMentionRow>,
    changes: Vec<SemanticChangeRow>,
    operations: Vec<SemanticOperationRow>,
    edges: Vec<Edge>,
}
impl FactRows {
    pub(super) fn digest(&self) -> GraphResult<String> {
        let mut digest = crate::processing::stable_digest(&"kg-rows-v7")?;
        for row in self.iter_mutations() {
            digest = crate::processing::stable_digest(&(digest, row))?;
        }
        Ok(digest)
    }
    pub(super) fn mutations(&self) -> Vec<Mutation<'_>> {
        self.iter_mutations().collect()
    }
    fn iter_mutations(&self) -> impl Iterator<Item = Mutation<'_>> {
        self.files
            .iter()
            .map(Mutation::File)
            .chain(self.nodes.iter().map(Mutation::Node))
            .chain(self.references.iter().map(Mutation::Reference))
            .chain(self.anchors.iter().map(Mutation::Anchor))
            .chain(self.catalogs.iter().map(Mutation::Catalog))
            .chain(self.scopes.iter().map(Mutation::Scope))
            .chain(self.bindings.iter().map(Mutation::Binding))
            .chain(self.diagnostics.iter().map(Mutation::Diagnostic))
            .chain(self.semantic_sources.iter().map(Mutation::SemanticSource))
            .chain(self.semantic_entities.iter().map(Mutation::SemanticEntity))
            .chain(self.episodes.iter().map(Mutation::Episode))
            .chain(self.scalar_facts.iter().map(Mutation::ScalarFact))
            .chain(self.associations.iter().map(Mutation::Association))
            .chain(self.deferred_mentions.iter().map(Mutation::DeferredMention))
            .chain(self.changes.iter().map(Mutation::SemanticChange))
            .chain(self.operations.iter().map(Mutation::SemanticOperation))
            .chain(
                self.semantic_relations
                    .iter()
                    .map(Mutation::SemanticRelation),
            )
            .chain(self.edges.iter().map(Mutation::Edge))
    }
    fn edge(&mut self, name: &str, source: &str, target: &str, version: Option<String>) {
        self.edges.push(Edge {
            name: name.into(),
            source: source.into(),
            target: target.into(),
            version,
        });
    }
    #[expect(
        clippy::too_many_lines,
        reason = "the eight typed table mappings stay adjacent for schema auditing"
    )]
    pub(super) fn encode(catalog: &FactCatalog) -> GraphResult<Self> {
        let mut rows = Self::default();
        let resolutions: BTreeMap<_, _> = catalog
            .resolutions
            .iter()
            .map(|r| (r.reference_id.as_str(), r))
            .collect();
        for file in &catalog.files {
            let r = &file.record;
            let f = file.facts.as_ref();
            rows.files.push(FileRow {
                path: r.path.clone(),
                file_id: u64::from(r.file_id),
                index_status: tag(&r.index_status)?,
                content_hash: r.content_hash.clone(),
                size_bytes: r.size_bytes,
                text_revision: r.text_revision,
                storage_generation: r.storage_generation.clone(),
                entity_count: r.entity_count,
                entity_ids_digest: r.entity_ids_digest.clone(),
                facts_present: f.is_some(),
                language: f.map(|f| tag(&f.language)).transpose()?,
                raw_digest: f.map(|f| f.version.raw_digest.clone()),
                decoded_digest: f.map(|f| f.version.decoded_digest.clone()),
                namespace: f.and_then(|f| f.module.as_ref().map(|m| m.namespace.clone())),
                module_path: f
                    .and_then(|f| f.module.as_ref())
                    .map(|m| json(&m.path))
                    .transpose()?,
                is_package: f.and_then(|f| f.module.as_ref().map(|m| m.is_package)),
                rules_fingerprint: f.map(|f| f.rules_fingerprint.clone()),
                complete: f.map(|f| f.complete),
                facts_format_version: FACTS_FORMAT_VERSION,
                text_map: json(&file.text_map)?,
                text_binding_digest: file.text_binding_digest.clone(),
            });
            let Some(f) = f else {
                continue;
            };
            for (i, n) in f.nodes.iter().enumerate() {
                rows.nodes.push(NodeRow {
                    id: n.id.clone(),
                    file_path: r.path.clone(),
                    ordinal: number(i)?,
                    domain: if f.language == crate::relations::Language::Markdown {
                        "documents"
                    } else {
                        "code"
                    }
                    .into(),
                    kind: tag(&n.kind)?,
                    entity_kind: tag(&n.entity_kind)?,
                    name: n.name.clone(),
                    qualified_name: json(&n.qualified_name)?,
                    qualified_label: f
                        .module
                        .as_ref()
                        .map_or_else(Vec::new, |m| m.path.clone())
                        .iter()
                        .chain(n.qualified_name.iter())
                        .cloned()
                        .collect::<Vec<_>>()
                        .join(
                            if matches!(
                                f.language,
                                crate::relations::Language::Rust | crate::relations::Language::Cpp
                            ) {
                                "::"
                            } else {
                                "."
                            },
                        ),
                    parent_id: n.parent.clone(),
                    path_addressable: n.path_addressable,
                    callable: n.callable,
                    span_start: number(n.span.start)?,
                    span_end: number(n.span.end)?,
                    entity_ids: json(&file.node_entities[i])?,
                });
                if let Some(parent) = &n.parent {
                    rows.edge("KgContains", parent, &n.id, None);
                }
            }
            for (i, s) in f.scopes.iter().enumerate() {
                let id = synthetic_id(&r.path, "scope", i)?;
                rows.scopes.push(ScopeRow {
                    id: id.clone(),
                    file_path: r.path.clone(),
                    ordinal: number(i)?,
                    kind: tag(&s.kind)?,
                    owner_id: s.owner.clone(),
                });
                rows.edge("KgScopeOf", &id, &s.owner, None);
                if let Some(parent) = s.parent {
                    rows.edge(
                        "KgScopeParent",
                        &id,
                        &synthetic_id(&r.path, "scope", parent)?,
                        None,
                    );
                }
            }
            for (i, b) in f.bindings.iter().enumerate() {
                let id = synthetic_id(&r.path, "binding", i)?;
                let (kind, node, path, target_scope) = match &b.target {
                    BindingTarget::Node { id } => ("node", Some(id.clone()), None, None),
                    BindingTarget::Import { path } => ("import", None, Some(json(path)?), None),
                    BindingTarget::PackageImport { path, alias } => {
                        ("package_import", None, Some(json(&(path, alias))?), None)
                    }
                    BindingTarget::GlobImport { path } => {
                        ("glob_import", None, Some(json(path)?), None)
                    }
                    BindingTarget::Redirect { scope } => {
                        ("redirect", None, None, Some(number(*scope)?))
                    }
                    BindingTarget::Dynamic => ("dynamic", None, None, None),
                };
                rows.bindings.push(BindingRow {
                    id: id.clone(),
                    file_path: r.path.clone(),
                    ordinal: number(i)?,
                    scope_ordinal: number(b.scope)?,
                    name: b.name.clone(),
                    target_kind: kind.into(),
                    target_node_id: node.clone(),
                    target_import_path: path,
                    target_scope,
                    visibility: b.visibility.clone(),
                    visible_from: number(b.visible_from)?,
                });
                rows.edge(
                    "KgInScope",
                    &id,
                    &synthetic_id(&r.path, "scope", b.scope)?,
                    None,
                );
                if let Some(node) = &node {
                    rows.edge("KgBindNode", &id, node, None);
                }
            }
            for (i, reference) in f.references.iter().enumerate() {
                let resolution = resolutions.get(reference.id.as_str());
                let row = ReferenceRow {
                    id: reference.id.clone(),
                    file_path: r.path.clone(),
                    ordinal: number(i)?,
                    owner_id: reference.owner.clone(),
                    scope_ordinal: number(reference.scope)?,
                    kind: tag(&reference.kind)?,
                    status: resolution.map(|v| tag(&v.status)).transpose()?,
                    reason: resolution.map(|v| v.reason.clone()),
                    span_start: number(reference.span.start)?,
                    span_end: number(reference.span.end)?,
                    lookup: json(&reference.lookup)?,
                    entity_ids: json(&file.reference_entities[i])?,
                };
                if let Some(resolution) =
                    resolution.filter(|r| r.status == ResolutionStatus::Resolved)
                {
                    let [target] = resolution.targets.as_slice() else {
                        return Err(invalid());
                    };
                    let name = match reference.kind {
                        crate::relations::RelationKind::Call => "KgCalls",
                        crate::relations::RelationKind::Import => "KgImports",
                        crate::relations::RelationKind::DocumentLink => "KgLinks",
                        crate::relations::RelationKind::Extends => "KgExtends",
                        crate::relations::RelationKind::Implements => "KgImplements",
                    };
                    rows.edge(
                        name,
                        resolution
                            .relation_source
                            .as_ref()
                            .map_or(reference.owner.as_str(), |source| source.node_id.as_str()),
                        &target.node_id,
                        Some(json(&ResolvedReference {
                            relation_source: resolution.relation_source.clone(),
                            reference: row,
                            target: target.clone(),
                            catalog_revision: resolution.catalog_revision.clone(),
                        })?),
                    );
                } else {
                    rows.references.push(row);
                }
            }
            let mut discriminators = BTreeMap::<&str, u64>::new();
            for (i, a) in f.anchors.iter().enumerate() {
                let d = discriminators.entry(&a.name).or_default();
                let id = synthetic_id(&r.path, "anchor", i)?;
                rows.anchors.push(AnchorRow {
                    id: id.clone(),
                    file_path: r.path.clone(),
                    ordinal: number(i)?,
                    slug: a.name.clone(),
                    node_id: a.node_id.clone(),
                    discriminator: *d,
                });
                *d += 1;
                rows.edge("KgAnchorOf", &id, &a.node_id, None);
            }
            for (i, d) in f.diagnostics.iter().enumerate() {
                let id = synthetic_id(&r.path, "diagnostic", i)?;
                rows.diagnostics.push(DiagnosticRow {
                    id: id.clone(),
                    file_path: r.path.clone(),
                    ordinal: number(i)?,
                    code: d.code.clone(),
                    span_start: d.span.map(|s| number(s.start)).transpose()?,
                    span_end: d.span.map(|s| number(s.end)).transpose()?,
                });
                rows.edge("KgDiagOf", &id, &r.path, None);
            }
        }
        for c in &catalog.catalogs {
            rows.catalogs.push(CatalogRow {
                revision: c.revision.clone(),
                domain: c.key.domain.clone(),
                language: c.key.language.clone(),
                namespace: c.key.namespace.clone(),
                member_count: number(c.members.len())?,
                members_digest: c.members_digest.clone(),
                config_fingerprint: c.config_fingerprint.clone(),
                selection_fingerprint: c.selection_fingerprint.clone(),
            });
            for member in &c.members {
                rows.edge("KgMemberOf", member, &c.revision, None);
            }
        }
        rows.encode_semantic(&catalog.semantic)?;
        rows.sort();
        Ok(rows)
    }
    fn sort(&mut self) {
        self.files.sort_by(|a, b| a.path.cmp(&b.path));
        self.nodes.sort_by(|a, b| a.id.cmp(&b.id));
        self.scopes.sort_by(|a, b| a.id.cmp(&b.id));
        self.bindings.sort_by(|a, b| a.id.cmp(&b.id));
        self.references.sort_by(|a, b| a.id.cmp(&b.id));
        self.anchors.sort_by(|a, b| a.id.cmp(&b.id));
        self.diagnostics.sort_by(|a, b| a.id.cmp(&b.id));
        self.catalogs.sort_by(|a, b| a.revision.cmp(&b.revision));
        self.edges.sort();
        self.semantic_sources
            .sort_by(|a, b| a.source_id.cmp(&b.source_id));
        self.semantic_entities.sort_by(|a, b| a.id.cmp(&b.id));
        self.episodes.sort_by(|a, b| a.id.cmp(&b.id));
        self.scalar_facts.sort_by(|a, b| a.id.cmp(&b.id));
        self.associations.sort_by(|a, b| a.id.cmp(&b.id));
        self.deferred_mentions.sort_by(|a, b| a.id.cmp(&b.id));
        self.changes.sort_by(|a, b| a.id.cmp(&b.id));
        self.operations.sort_by(|a, b| a.id.cmp(&b.id));
        self.semantic_relations
            .sort_by(|a, b| (&a.source, &a.target, &a.id).cmp(&(&b.source, &b.target, &b.id)));
    }
    pub(super) fn cost(&self) -> usize {
        self.files.len()
            + self.nodes.len()
            + self.scopes.len()
            + self.bindings.len()
            + self.references.len()
            + self.anchors.len()
            + self.diagnostics.len()
            + self.catalogs.len()
            + self.edges.len()
            + self.semantic_sources.len()
            + self.semantic_entities.len()
            + self.episodes.len()
            + self.scalar_facts.len()
            + self.associations.len()
            + self.semantic_relations.len()
            + self.deferred_mentions.len()
            + self.changes.len()
            + self.operations.len()
    }

    /// Import one logical `ImportPlan` batch without changing its boundaries or
    /// mutation digest. Nodes precede edges, as in the existing mutation order.
    pub(super) fn import_batch(
        mutations: &[Mutation<'_>],
        tx: &mut Transaction<'_>,
        options: &QueryOptions,
        context: &ProcessingContext,
    ) -> GraphResult<()> {
        macro_rules! nodes {
            ($variant:ident, $row:ident) => {{
                let selected = mutations
                    .iter()
                    .filter_map(|mutation| match mutation {
                        Mutation::$variant(row) => Some(*row),
                        _ => None,
                    })
                    .collect::<Vec<_>>();
                context.check()?;
                $row::import(&selected, tx, options)?;
            }};
        }
        for mutation in mutations {
            context.check()?;
            if let Mutation::Edge(edge) = mutation {
                RELATIONS
                    .iter()
                    .find(|relation| relation.0 == edge.name)
                    .ok_or_else(invalid)?;
            }
        }
        // Source TextMap strings can exceed COPY's 128 KiB record limit.
        // Parameterized inserts share this logical transaction and its budget.
        for mutation in mutations {
            if let Mutation::File(row) = mutation {
                context.check()?;
                row.insert(tx, options)?;
            }
        }
        nodes!(Node, NodeRow);
        nodes!(Reference, ReferenceRow);
        nodes!(Anchor, AnchorRow);
        nodes!(Catalog, CatalogRow);
        nodes!(Scope, ScopeRow);
        nodes!(Binding, BindingRow);
        nodes!(Diagnostic, DiagnosticRow);
        for mutation in mutations {
            context.check()?;
            match mutation {
                Mutation::SemanticSource(row) => row.insert(tx, options)?,
                Mutation::SemanticEntity(row) => row.insert(tx, options)?,
                Mutation::Episode(row) => row.insert(tx, options)?,
                Mutation::ScalarFact(row) => row.insert(tx, options)?,
                Mutation::Association(row) => row.insert(tx, options)?,
                Mutation::SemanticRelation(row) => row.insert(tx, options)?,
                Mutation::DeferredMention(row) => row.insert(tx, options)?,
                Mutation::SemanticChange(row) => row.insert(tx, options)?,
                Mutation::SemanticOperation(row) => row.insert(tx, options)?,
                _ => {}
            }
        }
        for &(name, source, target, properties) in RELATIONS {
            let edges = mutations
                .iter()
                .filter_map(|mutation| match mutation {
                    Mutation::Edge(edge) if edge.name == name => Some(*edge),
                    _ => None,
                })
                .collect::<Vec<_>>();
            context.check()?;
            Edge::import(
                &edges,
                name,
                source,
                target,
                !properties.is_empty(),
                tx,
                options,
            )?;
        }
        context.check()?;
        Ok(())
    }

    /// `include_structure=false` is exclusively a test baseline, not a usable
    /// production schema or an alternate fact backend. It writes no metadata.
    pub(super) fn write(
        &self,
        tx: &mut Transaction<'_>,
        options: &QueryOptions,
        context: &ProcessingContext,
        include_structure: bool,
    ) -> GraphResult<()> {
        macro_rules! insert { ($($list:ident),+) => { $(for row in &self.$list { context.check()?; row.insert(tx,options)?; })+ }; }
        insert!(files, nodes, references, anchors, catalogs);
        if include_structure {
            insert!(scopes, bindings, diagnostics);
        }
        insert!(
            semantic_sources,
            semantic_entities,
            episodes,
            scalar_facts,
            associations,
            deferred_mentions,
            changes,
            operations,
            semantic_relations
        );
        for edge in &self.edges {
            context.check()?;
            if !include_structure && STRUCTURE_RELATIONS.contains(&edge.name.as_str()) {
                continue;
            }
            edge.write(tx, options)?;
        }
        Ok(())
    }
    pub(super) fn read(tx: &mut Transaction<'_>, options: &QueryOptions) -> GraphResult<Self> {
        let _stage = profile::stage("fact_rows.read");
        profile::count("fact_rows.full_reads", 1);
        let mut rows = Self {
            files: FileRow::read(tx, options)?,
            nodes: NodeRow::read(tx, options)?,
            scopes: ScopeRow::read(tx, options)?,
            bindings: BindingRow::read(tx, options)?,
            references: ReferenceRow::read(tx, options)?,
            anchors: AnchorRow::read(tx, options)?,
            diagnostics: DiagnosticRow::read(tx, options)?,
            catalogs: CatalogRow::read(tx, options)?,
            semantic_sources: SemanticSourceRow::read(tx, options)?,
            semantic_entities: SemanticEntityRow::read(tx, options)?,
            episodes: EpisodeRow::read(tx, options)?,
            scalar_facts: ScalarFactRow::read(tx, options)?,
            associations: AssociationRow::read(tx, options)?,
            deferred_mentions: DeferredMentionRow::read(tx, options)?,
            changes: SemanticChangeRow::read(tx, options)?,
            operations: SemanticOperationRow::read(tx, options)?,
            semantic_relations: SemanticRelationRow::read(tx, options)?,
            edges: Vec::new(),
        };
        for (name, source, target, props) in RELATIONS {
            let source_key = if *source == "KgFile" { "path" } else { "id" };
            let target_key = if *target == "KgFile" {
                "path"
            } else if *target == "KgCatalogRevision" {
                "revision"
            } else {
                "id"
            };
            let version = if props.is_empty() {
                ""
            } else {
                ",e.data,e.ref_id"
            };
            let result = tx.query_with(&format!("MATCH (a:{source})-[e:{name}]->(b:{target}) RETURN a.{source_key},b.{target_key}{version}"), (), options)?;
            let mut cursor = result.rows();
            while let Some(r) = cursor.next()? {
                let payload = if props.is_empty() {
                    None
                } else {
                    let data = r.get_ref(2)?.as_str()?;
                    if edge_reference_id(name, data)? != r.get_ref(3)?.as_str()? {
                        return Err(invalid());
                    }
                    Some(data.to_owned())
                };
                rows.edge(
                    name,
                    r.get_ref(0)?.as_str()?,
                    r.get_ref(1)?.as_str()?,
                    payload,
                );
            }
        }
        rows.sort();
        if rows.cost() > options.max_results {
            return Err(GraphError::local(GraphErrorKind::ResourceExhausted));
        }
        profile::count("fact_rows.read_rows", number(rows.cost())?);
        Ok(rows)
    }

    pub(super) fn counts(&self) -> GraphResult<StoreCounts> {
        let mut nodes = BTreeMap::new();
        macro_rules! count { ($($table:literal => $field:ident),+) => { $(nodes.insert($table.into(),number(self.$field.len())?);)+ }; }
        count!("KgFile"=>files,"KgEntity"=>nodes,"KgScope"=>scopes,"KgBinding"=>bindings,"KgUnresolvedRef"=>references,"KgAnchor"=>anchors,"KgDiagnostic"=>diagnostics,"KgCatalogRevision"=>catalogs);
        count!("KgSemanticSource"=>semantic_sources,"KgSemanticEntity"=>semantic_entities,"KgEpisode"=>episodes,"KgScalarFact"=>scalar_facts,"KgSourceAssociation"=>associations,"KgDeferredMention"=>deferred_mentions,"KgSemanticChange"=>changes,"KgSemanticOperation"=>operations);
        let mut relations: BTreeMap<_, u64> =
            RELATIONS.iter().map(|r| (r.0.to_owned(), 0)).collect();
        for e in &self.edges {
            *relations.get_mut(&e.name).ok_or_else(invalid)? += 1;
        }
        relations.insert(
            "KgSemanticRelation".into(),
            number(self.semantic_relations.len())?,
        );
        Ok(StoreCounts { nodes, relations })
    }

    #[expect(
        clippy::too_many_lines,
        reason = "keep typed source reconstruction and ordinal checks together"
    )]
    fn decode_files(&self, context: &ProcessingContext) -> GraphResult<Vec<BoundFileFacts>> {
        let mut files = Vec::new();
        macro_rules! by_file {
            ($field:ident) => {{
                let mut index: BTreeMap<&str, Vec<_>> = BTreeMap::new();
                for row in &self.$field {
                    context.check()?;
                    index.entry(row.file_path.as_str()).or_default().push(row);
                }
                for rows in index.values_mut() {
                    rows.sort_by_key(|row| row.ordinal);
                }
                index
            }};
        }
        let nodes = by_file!(nodes);
        let scopes = by_file!(scopes);
        let bindings = by_file!(bindings);
        let anchors = by_file!(anchors);
        let diagnostics = by_file!(diagnostics);
        let mut resolved = Vec::new();
        for edge in &self.edges {
            context.check()?;
            profile::count("decode.edge_rows_examined", 1);
            if let Some(data) = &edge.version
                && edge.name != "KgMentions"
            {
                profile::count("decode.edge_json_parses", 1);
                resolved.push(unjson::<ResolvedReference>(data)?);
            }
        }
        let mut references: BTreeMap<&str, Vec<&ReferenceRow>> = BTreeMap::new();
        for row in self
            .references
            .iter()
            .chain(resolved.iter().map(|r| &r.reference))
        {
            context.check()?;
            references
                .entry(row.file_path.as_str())
                .or_default()
                .push(row);
        }
        for rows in references.values_mut() {
            rows.sort_by_key(|row| row.ordinal);
        }
        // Scope-parent reads are indexed once for reconstruction. Scanning the
        // entire edge set for each scope would turn a full read into S × E.
        let mut parents_by_scope: BTreeMap<&str, Vec<&Edge>> = BTreeMap::new();
        for edge in self.edges.iter().filter(|e| e.name == "KgScopeParent") {
            parents_by_scope.entry(&edge.source).or_default().push(edge);
        }
        let scopes_by_id: BTreeMap<_, _> = self.scopes.iter().map(|s| (s.id.as_str(), s)).collect();
        for row in &self.files {
            if row.facts_format_version != FACTS_FORMAT_VERSION {
                return Err(invalid());
            }
            let record = FileRecord {
                path: row.path.clone(),
                file_id: u32::try_from(row.file_id).map_err(|_| invalid())?,
                index_status: untag(&row.index_status)?,
                content_hash: row.content_hash.clone(),
                size_bytes: row.size_bytes,
                text_revision: row.text_revision,
                storage_generation: row.storage_generation.clone(),
                entity_count: row.entity_count,
                entity_ids_digest: row.entity_ids_digest.clone(),
            };
            if !row.facts_present {
                files.push(BoundFileFacts::inventory_only(record, context)?);
                continue;
            }
            let language = untag(row.language.as_deref().ok_or_else(invalid)?)?;
            let module = match (&row.namespace, &row.module_path, row.is_package) {
                (Some(namespace), Some(path), Some(is_package)) => Some(CodeModule {
                    namespace: namespace.clone(),
                    path: unjson(path)?,
                    is_package,
                }),
                (None, None, None) => None,
                _ => return Err(invalid()),
            };
            let mut facts = FileFacts {
                version: SourceVersion {
                    generation: row.storage_generation.clone(),
                    path: row.path.clone(),
                    raw_digest: row.raw_digest.clone().ok_or_else(invalid)?,
                    decoded_digest: row.decoded_digest.clone().ok_or_else(invalid)?,
                },
                language,
                module,
                rules_fingerprint: row.rules_fingerprint.clone().ok_or_else(invalid)?,
                complete: row.complete.ok_or_else(invalid)?,
                nodes: Vec::new(),
                scopes: Vec::new(),
                bindings: Vec::new(),
                references: Vec::new(),
                anchors: Vec::new(),
                diagnostics: Vec::new(),
            };
            let mut node_entities = Vec::new();
            let mut reference_entities = Vec::new();
            macro_rules! ordered {
                ($field:ident) => {{
                    let selected = $field.get(row.path.as_str()).map_or(&[][..], Vec::as_slice);
                    if selected
                        .iter()
                        .enumerate()
                        .any(|(i, r)| number(i).ok() != Some(r.ordinal))
                    {
                        return Err(invalid());
                    }
                    selected
                }};
            }
            for n in ordered!(nodes) {
                facts.nodes.push(FactNode {
                    id: n.id.clone(),
                    kind: untag(&n.kind)?,
                    entity_kind: untag(&n.entity_kind)?,
                    name: n.name.clone(),
                    qualified_name: unjson(&n.qualified_name)?,
                    parent: n.parent_id.clone(),
                    path_addressable: n.path_addressable,
                    callable: n.callable,
                    span: Span {
                        start: ordinal(n.span_start)?,
                        end: ordinal(n.span_end)?,
                    },
                    entity_indices: Vec::new(),
                });
                node_entities.push(unjson(&n.entity_ids)?);
            }
            for s in ordered!(scopes) {
                let parents = parents_by_scope
                    .get(s.id.as_str())
                    .map_or(&[][..], Vec::as_slice);
                let parent = match parents {
                    [] => None,
                    [e] => Some(ordinal(
                        scopes_by_id
                            .get(e.target.as_str())
                            .filter(|p| p.file_path == row.path)
                            .ok_or_else(invalid)?
                            .ordinal,
                    )?),
                    _ => return Err(invalid()),
                };
                facts.scopes.push(Scope {
                    parent,
                    owner: s.owner_id.clone(),
                    kind: untag(&s.kind)?,
                });
            }
            for b in ordered!(bindings) {
                let target = match (
                    b.target_kind.as_str(),
                    &b.target_node_id,
                    &b.target_import_path,
                    b.target_scope,
                ) {
                    ("node", Some(id), None, None) => BindingTarget::Node { id: id.clone() },
                    ("import", None, Some(path), None) => BindingTarget::Import {
                        path: unjson(path)?,
                    },
                    ("package_import", None, Some(value), None) => {
                        let (path, alias) = unjson(value)?;
                        BindingTarget::PackageImport { path, alias }
                    }
                    ("glob_import", None, Some(path), None) => BindingTarget::GlobImport {
                        path: unjson(path)?,
                    },
                    ("redirect", None, None, Some(scope)) => BindingTarget::Redirect {
                        scope: ordinal(scope)?,
                    },
                    ("dynamic", None, None, None) => BindingTarget::Dynamic,
                    _ => return Err(invalid()),
                };
                facts.bindings.push(Binding {
                    scope: ordinal(b.scope_ordinal)?,
                    name: b.name.clone(),
                    target,
                    visible_from: ordinal(b.visible_from)?,
                    visibility: b.visibility.clone(),
                });
            }
            for r in ordered!(references) {
                facts.references.push(Reference {
                    id: r.id.clone(),
                    owner: r.owner_id.clone(),
                    scope: ordinal(r.scope_ordinal)?,
                    kind: untag(&r.kind)?,
                    span: Span {
                        start: ordinal(r.span_start)?,
                        end: ordinal(r.span_end)?,
                    },
                    lookup: unjson(&r.lookup)?,
                    entity_indices: Vec::new(),
                });
                reference_entities.push(unjson(&r.entity_ids)?);
            }
            for a in ordered!(anchors) {
                facts.anchors.push(Anchor {
                    name: a.slug.clone(),
                    node_id: a.node_id.clone(),
                });
            }
            for d in ordered!(diagnostics) {
                let span = match (d.span_start, d.span_end) {
                    (Some(start), Some(end)) => Some(Span {
                        start: ordinal(start)?,
                        end: ordinal(end)?,
                    }),
                    (None, None) => None,
                    _ => return Err(invalid()),
                };
                facts.diagnostics.push(FactDiagnostic {
                    code: d.code.clone(),
                    span,
                });
            }
            files.push(BoundFileFacts {
                record,
                facts: Some(facts),
                node_entities,
                reference_entities,
                text_map: unjson(&row.text_map)?,
                text_binding_digest: row.text_binding_digest.clone(),
            });
        }
        Ok(files)
    }

    pub(super) fn decode(
        &self,
        metadata: &MetadataRow,
        context: &ProcessingContext,
    ) -> GraphResult<FactCatalog> {
        let _stage = profile::stage("fact_rows.decode");
        // Reconstruction indices and parsed edge payloads are released before
        // resolution and round-trip encoding allocate their complete results.
        let files = profile::measure("fact_rows.reconstruct", || self.decode_files(context))?;
        // Re-run the SAME pure resolver on persisted structure. Re-encoding
        // validates every typed property, ordinal, edge, target version, catalog
        // dependency and diagnostic; no source reread or second Cypher resolver.
        let mut catalog = profile::measure("fact_rows.resolve", || {
            FactCatalog::resolve(
                files,
                metadata.config_fingerprint.clone(),
                metadata.selection_fingerprint.clone(),
                context,
            )
        })?;
        catalog.semantic = self.decode_semantic()?;
        catalog.validate_semantic(context)?;
        if profile::measure("fact_rows.reencode", || Self::encode(&catalog))? != *self {
            return Err(invalid());
        }
        Ok(catalog)
    }
}

fn write_edge(
    output: &mut Vec<u8>,
    name: &str,
    source: &str,
    target: &str,
    payload: Option<&str>,
) -> GraphResult<()> {
    // Source-edge field order is fixed by the v7 canonical encoding.
    output.extend_from_slice(b"{\"name\":");
    serde_json::to_writer(&mut *output, name).map_err(|_| invalid())?;
    output.extend_from_slice(b",\"source\":");
    serde_json::to_writer(&mut *output, source).map_err(|_| invalid())?;
    output.extend_from_slice(b",\"target\":");
    serde_json::to_writer(&mut *output, target).map_err(|_| invalid())?;
    output.extend_from_slice(b",\"version\":");
    serde_json::to_writer(&mut *output, &payload).map_err(|_| invalid())?;
    output.push(b'}');
    Ok(())
}

fn record_scan_stats(scan: &::graphdb::OrderedScan<'_>) {
    let stats = scan.stats();
    super::super::profile::count("integrity.scans", 1);
    super::super::profile::count("integrity.scanned_rows", stats.scanned_rows);
    super::super::profile::count("integrity.sort_comparisons", stats.sort_comparisons);
    super::super::profile::peak("integrity.scan_key_bytes_peak", stats.retained_bytes);
}

impl super::GraphDbStore {
    /// Full integrity verification without reconstructing the fact catalog
    /// or running a second resolver on the public query path.
    pub(crate) fn verify_rows(&self, context: &ProcessingContext) -> GraphResult<String> {
        self.with_read(|tx, options| {
            let mut digest = crate::processing::stable_digest(&"kg-rows-v7")?;
            let mut count = 0usize;
            let mut scratch = Vec::new();
            let mut canonical_bytes = 0u64;
            macro_rules! scan {
                ($row:ident, $variant:ident, $key:ident) => {{
                    let _stage =
                        super::super::profile::stage(concat!("integrity.", stringify!($row)));
                    let mut scan = $row::scan(tx, options)?;
                    record_scan_stats(&scan);
                    while let Some(page) = scan.next_batch()? {
                        context.check()?;
                        super::super::profile::count("integrity.pages", 1);
                        let mut cursor = page.rows();
                        while let Some(raw) = cursor.next()? {
                            context.check()?;
                            count += 1;
                            context.check_output(count)?;
                            digest = chained_digest(
                                &digest,
                                &mut scratch,
                                stringify!($variant),
                                |buffer| $row::write_raw(&raw, buffer),
                            )?;
                            canonical_bytes = canonical_bytes.saturating_add(scratch.len() as u64);
                        }
                    }
                }};
            }
            scan!(FileRow, File, path);
            scan!(NodeRow, Node, id);
            scan!(ReferenceRow, Reference, id);
            scan!(AnchorRow, Anchor, id);
            scan!(CatalogRow, Catalog, revision);
            scan!(ScopeRow, Scope, id);
            scan!(BindingRow, Binding, id);
            scan!(DiagnosticRow, Diagnostic, id);
            scan!(SemanticSourceRow, SemanticSource, source_id);
            scan!(SemanticEntityRow, SemanticEntity, id);
            scan!(EpisodeRow, Episode, id);
            scan!(ScalarFactRow, ScalarFact, id);
            scan!(AssociationRow, Association, id);
            scan!(DeferredMentionRow, DeferredMention, id);
            scan!(SemanticChangeRow, SemanticChange, id);
            scan!(SemanticOperationRow, SemanticOperation, id);
            canonical_bytes += verify_semantic_relations(
                tx,
                options,
                context,
                &mut count,
                &mut digest,
                &mut scratch,
            )?;
            canonical_bytes += verify_reference_edges(
                tx,
                options,
                context,
                &mut count,
                &mut digest,
                &mut scratch,
            )?;
            super::super::profile::count("integrity.canonical_bytes", canonical_bytes);
            Ok(digest)
        })
    }
}

// Ordered edge scans accept only non-null STRING properties. Preserve native
// timestamps and nullable predecessors with one row-limited typed query.
fn verify_semantic_relations(
    tx: &mut Transaction<'_>,
    options: &QueryOptions,
    context: &ProcessingContext,
    count: &mut usize,
    digest: &mut String,
    scratch: &mut Vec<u8>,
) -> GraphResult<u64> {
    let mut canonical_bytes = 0_u64;
    let semantic = SemanticRelationRow::query(tx, options)?;
    let mut cursor = semantic.rows();
    while let Some(row) = cursor.next()? {
        context.check()?;
        *count += 1;
        context.check_output(*count)?;
        *digest = chained_digest(digest, scratch, "SemanticRelation", |buffer| {
            SemanticRelationRow::write_raw(&row, buffer)
        })?;
        canonical_bytes = canonical_bytes.saturating_add(scratch.len() as u64);
    }
    Ok(canonical_bytes)
}

fn verify_reference_edges(
    tx: &mut Transaction<'_>,
    options: &QueryOptions,
    context: &ProcessingContext,
    count: &mut usize,
    digest: &mut String,
    scratch: &mut Vec<u8>,
) -> GraphResult<u64> {
    let mut canonical_bytes = 0_u64;
    let mut tables = RELATIONS.to_vec();
    tables.sort_by_key(|r| r.0);
    for (name, source, target, props) in tables {
        let source_key = if source == "KgFile" { "path" } else { "id" };
        let target_key = match target {
            "KgFile" => "path",
            "KgCatalogRevision" => "revision",
            _ => "id",
        };
        let _stage = super::super::profile::stage("integrity.relationships");
        let properties: &[&str] = if props.is_empty() {
            &[]
        } else {
            &["data", "ref_id"]
        };
        let mut scan = tx.scan_edges(
            name,
            (source, source_key),
            (target, target_key),
            properties,
            options,
        )?;
        record_scan_stats(&scan);
        while let Some(result) = scan.next_batch()? {
            context.check()?;
            super::super::profile::count("integrity.pages", 1);
            let mut cursor = result.rows();
            while let Some(row) = cursor.next()? {
                context.check()?;
                *count += 1;
                context.check_output(*count)?;
                let payload = if props.is_empty() {
                    None
                } else {
                    Some(row.get_ref(2)?.as_str()?)
                };
                if let Some(payload) = payload
                    && edge_reference_id(name, payload)? != row.get_ref(3)?.as_str()?
                {
                    return Err(invalid());
                }
                let source = row.get_ref(0)?.as_str()?;
                let target = row.get_ref(1)?.as_str()?;
                *digest = chained_digest(digest, scratch, "Edge", |buffer| {
                    write_edge(buffer, name, source, target, payload)
                })?;
                canonical_bytes = canonical_bytes.saturating_add(scratch.len() as u64);
            }
        }
    }
    Ok(canonical_bytes)
}
