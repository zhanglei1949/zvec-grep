//! Typed fact rows and versioned JSON attributes; no AST/body storage.
use super::super::{
    GraphError, GraphErrorKind, GraphResult,
    schema::{
        AnchorRow, BindingRow, CatalogRow, DiagnosticRow, FileRow, MetadataRow, NodeRow, RELATIONS,
        ReferenceRow, STRUCTURE_RELATIONS, ScopeRow,
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
    /// The existing v4 digest chain, with the same canonical JSON bytes.
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
                            .map(|version| unjson::<ResolvedReference>(version))
                            .transpose()
                    })
                    .collect::<GraphResult<Vec<_>>>()?;
                columns.push(Column::strings(
                    resolved
                        .iter()
                        .map(|value| value.as_ref().map(|value| value.reference.id.clone()))
                        .collect(),
                ));
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
                params! {"source"=>&self.source,"target"=>&self.target,"version"=>version,"ref_id"=>&unjson::<ResolvedReference>(version)?.reference.id},
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
    edges: Vec<Edge>,
}
impl FactRows {
    pub(super) fn digest(&self) -> GraphResult<String> {
        let mut digest = crate::processing::stable_digest(&"kg-rows-v4")?;
        for row in self.mutations() {
            digest = crate::processing::stable_digest(&(digest, row))?;
        }
        Ok(digest)
    }
    pub(super) fn mutations(&self) -> Vec<Mutation<'_>> {
        let mut rows = Vec::with_capacity(self.cost());
        macro_rules! append {
            ($field:ident,$variant:ident) => {
                rows.extend(self.$field.iter().map(Mutation::$variant));
            };
        }
        append!(files, File);
        append!(nodes, Node);
        append!(references, Reference);
        append!(anchors, Anchor);
        append!(catalogs, Catalog);
        append!(scopes, Scope);
        append!(bindings, Binding);
        append!(diagnostics, Diagnostic);
        append!(edges, Edge);
        rows
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
                        .join(if f.language == crate::relations::Language::Rust {
                            "::"
                        } else {
                            "."
                        }),
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
                    };
                    rows.edge(
                        name,
                        &reference.owner,
                        &target.node_id,
                        Some(json(&ResolvedReference {
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
        nodes!(File, FileRow);
        nodes!(Node, NodeRow);
        nodes!(Reference, ReferenceRow);
        nodes!(Anchor, AnchorRow);
        nodes!(Catalog, CatalogRow);
        nodes!(Scope, ScopeRow);
        nodes!(Binding, BindingRow);
        nodes!(Diagnostic, DiagnosticRow);
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
        let mut rows = Self {
            files: FileRow::read(tx, options)?,
            nodes: NodeRow::read(tx, options)?,
            scopes: ScopeRow::read(tx, options)?,
            bindings: BindingRow::read(tx, options)?,
            references: ReferenceRow::read(tx, options)?,
            anchors: AnchorRow::read(tx, options)?,
            diagnostics: DiagnosticRow::read(tx, options)?,
            catalogs: CatalogRow::read(tx, options)?,
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
                if !props.is_empty()
                    && unjson::<ResolvedReference>(&r.try_get::<String>(2)?)?
                        .reference
                        .id
                        != r.try_get::<String>(3)?
                {
                    return Err(invalid());
                }
                rows.edge(
                    name,
                    &r.try_get::<String>(0)?,
                    &r.try_get::<String>(1)?,
                    if props.is_empty() {
                        None
                    } else {
                        Some(r.try_get(2)?)
                    },
                );
            }
        }
        rows.sort();
        if rows.cost() > options.max_results {
            return Err(GraphError::local(GraphErrorKind::ResourceExhausted));
        }
        Ok(rows)
    }

    pub(super) fn counts(&self) -> GraphResult<StoreCounts> {
        let mut nodes = BTreeMap::new();
        macro_rules! count { ($($table:literal => $field:ident),+) => { $(nodes.insert($table.into(),number(self.$field.len())?);)+ }; }
        count!("KgFile"=>files,"KgEntity"=>nodes,"KgScope"=>scopes,"KgBinding"=>bindings,"KgUnresolvedRef"=>references,"KgAnchor"=>anchors,"KgDiagnostic"=>diagnostics,"KgCatalogRevision"=>catalogs);
        let mut relations: BTreeMap<_, u64> =
            RELATIONS.iter().map(|r| (r.0.to_owned(), 0)).collect();
        for e in &self.edges {
            *relations.get_mut(&e.name).ok_or_else(invalid)? += 1;
        }
        Ok(StoreCounts { nodes, relations })
    }

    #[expect(
        clippy::too_many_lines,
        reason = "reconstruct ordered structure and verify every redundant relation before accepting it"
    )]
    pub(super) fn decode(
        &self,
        metadata: &MetadataRow,
        context: &ProcessingContext,
    ) -> GraphResult<FactCatalog> {
        let mut files = Vec::new();
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
                    let mut selected = self
                        .$field
                        .iter()
                        .filter(|v| v.file_path == row.path)
                        .collect::<Vec<_>>();
                    selected.sort_by_key(|r| r.ordinal);
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
            let mut reference_rows = self
                .references
                .iter()
                .filter(|r| r.file_path == row.path)
                .cloned()
                .collect::<Vec<_>>();
            for edge in &self.edges {
                if let Some(data) = &edge.version {
                    let resolved: ResolvedReference = unjson(data)?;
                    if resolved.reference.file_path == row.path {
                        reference_rows.push(resolved.reference);
                    }
                }
            }
            reference_rows.sort_by_key(|r| r.ordinal);
            if reference_rows
                .iter()
                .enumerate()
                .any(|(i, r)| number(i).ok() != Some(r.ordinal))
            {
                return Err(invalid());
            }
            for r in &reference_rows {
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
        // Re-run the SAME pure resolver on persisted structure. Re-encoding
        // validates every typed property, ordinal, edge, target version, catalog
        // dependency and diagnostic; no source reread or second Cypher resolver.
        let catalog = FactCatalog::resolve(
            files,
            metadata.config_fingerprint.clone(),
            metadata.selection_fingerprint.clone(),
            context,
        )?;
        if Self::encode(&catalog)? != *self {
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
    // Field order is part of the existing v4 canonical encoding.
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
    pub(crate) fn rows_digest(catalog: &FactCatalog) -> GraphResult<String> {
        FactRows::encode(catalog)?.digest()
    }

    /// Full, bounded-memory integrity verification. Does not reconstruct facts
    /// or run a second resolver on the public query path.
    pub(crate) fn verify_rows(&self, context: &ProcessingContext) -> GraphResult<String> {
        self.with_read(|tx, options| {
            let mut digest = crate::processing::stable_digest(&"kg-rows-v4")?;
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
                        count += 1;
                        context.check_output(count)?;
                        let payload = if props.is_empty() {
                            None
                        } else {
                            Some(row.get_ref(2)?.as_str()?)
                        };
                        if let Some(payload) = payload
                            && unjson::<ResolvedReference>(payload)?.reference.id
                                != row.get_ref(3)?.as_str()?
                        {
                            return Err(invalid());
                        }
                        let source = row.get_ref(0)?.as_str()?;
                        let target = row.get_ref(1)?.as_str()?;
                        digest = chained_digest(&digest, &mut scratch, "Edge", |buffer| {
                            write_edge(buffer, name, source, target, payload)
                        })?;
                        canonical_bytes = canonical_bytes.saturating_add(scratch.len() as u64);
                    }
                }
            }
            super::super::profile::count("integrity.canonical_bytes", canonical_bytes);
            Ok(digest)
        })
    }
}
