//! Graph-side commit proof only. A confirmed root is NOT a selected HEAD or
//! graph-ready result; engine still owns text fencing and publication.
use super::{
    GraphDbStore, GraphError, GraphErrorKind, GraphOutcome, GraphResult,
    schema::{self, MetadataRow, ReceiptRow, RootRow},
};
use crate::{
    processing::{ProcessingContext, stable_digest},
    relations::{FACT_CONTRACT_VERSION, bound::FactCatalog},
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BuildScope {
    Code,
    Documents,
    Both,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BuildSpec {
    pub graph_build_id: String,
    pub operation_id: String,
    pub attempt: u64,
    pub base_text_revision: u64,
    pub base_catalog_digest: String,
    pub base_head: Option<String>,
    pub scope: BuildScope,
    pub configuration: serde_json::Value,
    pub policy_fingerprint: String,
    pub rules_fingerprint: String,
    pub module_roots_fingerprint: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConfirmedBuild {
    pub graph_build_id: String,
    pub receipt_id: String,
    pub facts_digest: String,
    pub committed_epoch_ms: u64,
}

fn invalid() -> GraphError {
    GraphError::local(GraphErrorKind::InvalidInput)
}
fn uuid(id: &str) -> bool {
    id.len() == 36
        && id.bytes().enumerate().all(|(i, b)| {
            if [8, 13, 18, 23].contains(&i) {
                b == b'-'
            } else {
                b.is_ascii_hexdigit()
            }
        })
}
fn digest(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|b| b.is_ascii_hexdigit())
}
impl BuildSpec {
    fn root(&self, catalog: &FactCatalog) -> GraphResult<RootRow> {
        if !uuid(&self.graph_build_id)
            || !uuid(&self.operation_id)
            || self.base_head.as_ref().is_some_and(|id| !uuid(id))
            || self.attempt == 0
            || ![
                &self.base_catalog_digest,
                &self.policy_fingerprint,
                &self.rules_fingerprint,
                &self.module_roots_fingerprint,
            ]
            .into_iter()
            .all(|d| digest(d))
            || !self.configuration.is_object()
            || stable_digest(&self.configuration)? != self.policy_fingerprint
            || catalog.config_fingerprint != self.policy_fingerprint
            || catalog
                .files
                .iter()
                .any(|f| f.record.text_revision > self.base_text_revision)
            || catalog
                .files
                .iter()
                .filter_map(|f| f.facts.as_ref())
                .any(|f| match self.scope {
                    BuildScope::Code => f.language == crate::relations::Language::Markdown,
                    BuildScope::Documents => f.language != crate::relations::Language::Markdown,
                    BuildScope::Both => false,
                })
        {
            return Err(invalid());
        }
        let (nodes, references) = catalog
            .files
            .iter()
            .filter_map(|f| f.facts.as_ref())
            .fold((0_u64, 0_u64), |(n, r), f| {
                (n + f.nodes.len() as u64, r + f.references.len() as u64)
            });
        Ok(RootRow {
            graph_build_id: self.graph_build_id.clone(),
            operation_id: self.operation_id.clone(),
            attempt: self.attempt,
            base_text_revision: self.base_text_revision,
            base_catalog_digest: self.base_catalog_digest.clone(),
            base_head: self.base_head.clone(),
            scope: match self.scope {
                BuildScope::Code => "code",
                BuildScope::Documents => "documents",
                BuildScope::Both => "both",
            }
            .into(),
            contract_version: FACT_CONTRACT_VERSION.into(),
            schema_fingerprint: schema::build_fingerprint()?,
            rules_fingerprint: self.rules_fingerprint.clone(),
            module_roots_fingerprint: self.module_roots_fingerprint.clone(),
            configuration: serde_json::to_string(&self.configuration).map_err(|_| invalid())?,
            policy_fingerprint: self.policy_fingerprint.clone(),
            catalog_revisions: serde_json::to_string(&catalog.catalogs).map_err(|_| invalid())?,
            facts_digest: stable_digest(catalog)?,
            rows_digest: GraphDbStore::rows_digest(catalog)?,
            file_count: catalog.files.len() as u64,
            node_count: nodes,
            reference_count: references,
            committed_epoch_ms: 0,
        })
    }
}
fn receipt(root: &RootRow) -> GraphResult<ReceiptRow> {
    Ok(ReceiptRow {
        receipt_id: stable_digest(&(&root.graph_build_id, &root.operation_id, root.attempt))?,
        graph_build_id: root.graph_build_id.clone(),
        operation_id: root.operation_id.clone(),
        attempt: root.attempt,
        root_digest: stable_digest(root)?,
        outcome: "committed".into(),
        stage: "commit".into(),
        detail_code: String::new(),
        epoch_ms: root.committed_epoch_ms,
    })
}
fn proof(
    mut expected: RootRow,
    roots: &[RootRow],
    receipts: &[ReceiptRow],
) -> GraphResult<Option<ConfirmedBuild>> {
    if roots.is_empty() && receipts.is_empty() {
        return Ok(None);
    }
    let conflict = || GraphError::local(GraphErrorKind::Conflict);
    if roots.len() != 1 || receipts.len() != 1 {
        return Err(GraphError::local(GraphErrorKind::IncompatibleFormat));
    }
    if roots[0].schema_fingerprint != schema::build_fingerprint()?
        || roots[0].contract_version != FACT_CONTRACT_VERSION
    {
        return Err(GraphError::local(GraphErrorKind::IncompatibleFormat));
    }
    expected.committed_epoch_ms = roots[0].committed_epoch_ms;
    let success = receipt(&expected)?;
    if roots[0] != expected || receipts[0] != success {
        return Err(conflict());
    }
    Ok(Some(ConfirmedBuild {
        graph_build_id: expected.graph_build_id,
        receipt_id: success.receipt_id,
        facts_digest: expected.facts_digest,
        committed_epoch_ms: expected.committed_epoch_ms,
    }))
}
impl GraphDbStore {
    /// Partial fact imports cannot contain either publication record. This
    /// independent check does not require fact-ready metadata to exist.
    pub fn ensure_no_build_commit(&self) -> GraphResult<()> {
        self.with_read(|tx, options| {
            if !RootRow::read(tx, options)?.is_empty() || !ReceiptRow::read(tx, options)?.is_empty()
            {
                return Err(GraphError::local(GraphErrorKind::IncompatibleFormat));
            }
            Ok(())
        })
    }
    /// Commit an immutable root and success receipt in one short transaction.
    /// Identical retries reuse the committed proof; changed inputs conflict.
    pub fn commit_build(
        &self,
        spec: &BuildSpec,
        context: &ProcessingContext,
    ) -> GraphResult<ConfirmedBuild> {
        let _stage = super::profile::stage("graph.commit_build");
        let expected = spec.root(&self.read_facts(context)?)?;
        context.check_input(
            expected
                .configuration
                .len()
                .saturating_add(expected.catalog_revisions.len()),
        )?;
        context.check_output(2)?;
        context.check()?;
        self.with_write(|tx, options| {
            let metadata = MetadataRow::read(tx, options)?;
            if metadata.len() != 1
                || metadata[0].content_digest != expected.facts_digest
                || metadata[0].rows_digest != expected.rows_digest
            {
                return Err(GraphError::local(GraphErrorKind::Conflict));
            }
            if proof(
                expected.clone(),
                &RootRow::read(tx, options)?,
                &ReceiptRow::read(tx, options)?,
            )?
            .is_some()
            {
                return Ok(());
            }
            context.check()?;
            let mut root = expected.clone();
            root.committed_epoch_ms = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_err(|_| invalid())?
                .as_millis()
                .try_into()
                .map_err(|_| invalid())?;
            root.insert(tx, options)?;
            receipt(&root)?.insert(tx, options)?;
            Ok(())
        })?;
        // New explicit READ ONLY session, no post-commit cancellation override.
        self.with_read(|tx, options| {
            proof(
                expected,
                &RootRow::read(tx, options)?,
                &ReceiptRow::read(tx, options)?,
            )?
            .ok_or_else(|| GraphError::local(GraphErrorKind::Storage))
        })
        .map_err(|mut error| {
            error.outcome = Some(GraphOutcome::Committed);
            error.stage = "confirm".into();
            error
        })
    }

    /// Inspection/recovery proof only; this does not repair or select anything.
    pub fn inspect_build(
        &self,
        spec: &BuildSpec,
        context: &ProcessingContext,
    ) -> GraphResult<Option<ConfirmedBuild>> {
        let _stage = super::profile::stage("graph.inspect_build");
        let expected = spec.root(&self.read_facts(context)?)?;
        context.check_input(
            expected
                .configuration
                .len()
                .saturating_add(expected.catalog_revisions.len()),
        )?;
        context.check_output(2)?;
        self.with_read(|tx, options| {
            proof(
                expected,
                &RootRow::read(tx, options)?,
                &ReceiptRow::read(tx, options)?,
            )
        })
    }
}

impl GraphDbStore {
    /// Selected immutable candidate proof for query admission. Integrity scans
    /// pages without materializing a `FactCatalog`; building/recovery keep the
    /// existing semantic round-trip verification.
    pub fn inspect_published(
        &self,
        spec: &BuildSpec,
        context: &ProcessingContext,
    ) -> GraphResult<Option<ConfirmedBuild>> {
        let digest = self.verify_rows(context)?;
        self.with_read(|tx, options| {
            let roots = RootRow::read(tx, options)?;
            let [root] = roots.as_slice() else {
                return Ok(None);
            };
            let metadata = MetadataRow::read(tx, options)?;
            let [metadata] = metadata.as_slice() else {
                return Ok(None);
            };
            let mut expected = root.clone();
            expected.graph_build_id.clone_from(&spec.graph_build_id);
            expected.operation_id.clone_from(&spec.operation_id);
            expected.attempt = spec.attempt;
            expected.base_text_revision = spec.base_text_revision;
            expected
                .base_catalog_digest
                .clone_from(&spec.base_catalog_digest);
            expected.base_head.clone_from(&spec.base_head);
            expected.scope = match spec.scope {
                BuildScope::Code => "code",
                BuildScope::Documents => "documents",
                BuildScope::Both => "both",
            }
            .into();
            expected.contract_version = FACT_CONTRACT_VERSION.into();
            expected.schema_fingerprint = schema::build_fingerprint()?;
            expected
                .rules_fingerprint
                .clone_from(&spec.rules_fingerprint);
            expected
                .module_roots_fingerprint
                .clone_from(&spec.module_roots_fingerprint);
            expected.configuration =
                serde_json::to_string(&spec.configuration).map_err(|_| invalid())?;
            expected
                .policy_fingerprint
                .clone_from(&spec.policy_fingerprint);
            if root.rows_digest != digest
                || metadata.rows_digest != digest
                || metadata.content_digest != root.facts_digest
                || metadata.config_fingerprint != root.policy_fingerprint
                || stable_digest(&spec.configuration)? != spec.policy_fingerprint
            {
                return Err(invalid());
            }
            proof(expected, &roots, &ReceiptRow::read(tx, options)?)
        })
    }
}
