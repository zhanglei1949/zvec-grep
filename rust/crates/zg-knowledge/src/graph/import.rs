//! Bounded parameterized batches. Progress and mutations share each transaction;
//! only complete readback can install the normal fact-ready metadata.
use super::super::profile;
use super::super::schema::{self, ImportRow, MetadataRow};
use super::{
    GraphDbStore, GraphError, GraphErrorKind, GraphLimits, GraphResult, check_metadata,
    codec::{FactRows, Mutation},
};
use crate::{
    processing::{ProcessingContext, stable_digest},
    relations::bound::FactCatalog,
};
use graphdb::{QueryOptions, Transaction, params};
use serde::{Deserialize, Serialize};
use std::{ops::Range, path::Path};

const VERSION: u64 = 1;
fn invalid() -> GraphError {
    GraphError::local(GraphErrorKind::IncompatibleFormat)
}
fn conflict() -> GraphError {
    GraphError::local(GraphErrorKind::Conflict)
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImportBudget {
    /// Fact mutations per transaction; one additional progress mutation is fixed.
    pub max_batch_rows: usize,
    /// Serialized fact parameters, including JSON escaping. Native allocation
    /// and transaction duration are not implied to have this hard bound.
    pub max_batch_bytes: usize,
}
impl Default for ImportBudget {
    fn default() -> Self {
        Self {
            max_batch_rows: 256,
            max_batch_bytes: 256 * 1024,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportProgress {
    pub budget: ImportBudget,
    pub plan_digest: String,
    pub facts_digest: String,
    pub completed_batches: u64,
    pub total_batches: u64,
    pub committed_rows: u64,
    pub total_rows: u64,
    pub finalized: bool,
}
impl TryFrom<&ImportRow> for ImportProgress {
    type Error = GraphError;
    fn try_from(r: &ImportRow) -> GraphResult<Self> {
        Ok(Self {
            budget: ImportBudget {
                max_batch_rows: usize::try_from(r.max_batch_rows).map_err(|_| invalid())?,
                max_batch_bytes: usize::try_from(r.max_batch_bytes).map_err(|_| invalid())?,
            },
            plan_digest: r.plan_digest.clone(),
            facts_digest: r.facts_digest.clone(),
            completed_batches: r.completed_batches,
            total_batches: r.total_batches,
            committed_rows: r.committed_rows,
            total_rows: r.total_rows,
            finalized: r.finalized,
        })
    }
}
struct Batch {
    range: Range<usize>,
    prefix: String,
}
struct Plan {
    initial: ImportRow,
    batches: Vec<Batch>,
}
impl Plan {
    fn new(
        rows: &[Mutation<'_>],
        facts_digest: String,
        budget: &ImportBudget,
        context: &ProcessingContext,
    ) -> GraphResult<Self> {
        if budget.max_batch_rows == 0 || budget.max_batch_bytes == 0 {
            return Err(GraphError::local(GraphErrorKind::InvalidInput));
        }
        context.check_output(rows.len())?;
        let mut descriptors = Vec::new();
        let mut start = 0;
        let mut bytes = 0_usize;
        let mut total_bytes = 0_usize;
        let mut digests = Vec::new();
        for (i, row) in rows.iter().enumerate() {
            context.check()?;
            let encoded = serde_json::to_vec(row).map_err(|_| invalid())?;
            if encoded.len() > budget.max_batch_bytes {
                return Err(GraphError::local(GraphErrorKind::ResourceExhausted));
            }
            total_bytes = total_bytes.checked_add(encoded.len()).ok_or_else(invalid)?;
            context.check_input(total_bytes)?;
            if !digests.is_empty()
                && (i - start == budget.max_batch_rows
                    || bytes.saturating_add(encoded.len()) > budget.max_batch_bytes)
            {
                descriptors.push((start..i, bytes, stable_digest(&digests)?));
                start = i;
                bytes = 0;
                digests.clear();
            }
            bytes += encoded.len();
            digests.push(zg_storage::sha256_hex(&encoded));
        }
        if !digests.is_empty() {
            descriptors.push((start..rows.len(), bytes, stable_digest(&digests)?));
        }
        let plan_digest = stable_digest(&(
            VERSION,
            &facts_digest,
            budget,
            &descriptors
                .iter()
                .map(|(r, b, d)| (r.start, r.end, b, d))
                .collect::<Vec<_>>(),
        ))?;
        let mut prefix = stable_digest(&(VERSION, "empty-import"))?;
        let initial = ImportRow {
            id: "facts".into(),
            format_version: VERSION,
            plan_digest,
            facts_digest,
            max_batch_rows: budget.max_batch_rows as u64,
            max_batch_bytes: budget.max_batch_bytes as u64,
            total_batches: descriptors.len() as u64,
            total_rows: rows.len() as u64,
            completed_batches: 0,
            committed_rows: 0,
            prefix_digest: prefix.clone(),
            finalized: false,
        };
        let mut batches = Vec::new();
        for (range, _, digest) in descriptors {
            prefix = stable_digest(&(&prefix, digest))?;
            batches.push(Batch {
                range,
                prefix: prefix.clone(),
            });
        }
        Ok(Self { initial, batches })
    }
    fn at(&self, complete: usize) -> ImportRow {
        let mut r = self.initial.clone();
        if complete > 0 {
            r.completed_batches = complete as u64;
            r.committed_rows = self.batches[complete - 1].range.end as u64;
            r.prefix_digest
                .clone_from(&self.batches[complete - 1].prefix);
        }
        r
    }
}
fn progress(tx: &mut Transaction<'_>, options: &QueryOptions) -> GraphResult<ImportRow> {
    let rows = ImportRow::read(tx, options)?;
    let [r] = rows.as_slice() else {
        return Err(invalid());
    };
    if r.id != "facts"
        || r.format_version != VERSION
        || r.max_batch_rows == 0
        || r.max_batch_bytes == 0
        || r.completed_batches > r.total_batches
        || r.committed_rows > r.total_rows
        || (r.finalized && r.completed_batches != r.total_batches)
    {
        return Err(invalid());
    }
    Ok(r.clone())
}
fn advance(tx: &mut Transaction<'_>, options: &QueryOptions, r: &ImportRow) -> GraphResult<()> {
    tx.execute_with("MATCH (p:KgImportProgress {id: $id}) SET p.completed_batches=$completed,p.committed_rows=$rows,p.prefix_digest=$prefix,p.finalized=$finalized",params!{"id"=>&r.id,"completed"=>r.completed_batches,"rows"=>r.committed_rows,"prefix"=>&r.prefix_digest,"finalized"=>r.finalized},options)?;
    Ok(())
}
impl GraphDbStore {
    pub fn create_batched_build(path: impl AsRef<Path>, limits: GraphLimits) -> GraphResult<Self> {
        let store = Self::create_build(path, limits)?;
        let mut c = store.database.connect()?;
        c.execute_with(&schema::import_statement(), (), &store.options())
            .map_err(|e| store.control_error(e.into()))?;
        c.close()?;
        Ok(store)
    }
    /// Explicit incomplete-candidate admission. Does not run DDL, mark ready,
    /// install a root or fix a progress marker. Use ordinary open for ready facts.
    pub fn open_import(
        path: impl AsRef<Path>,
        read_only: bool,
        limits: GraphLimits,
        context: &ProcessingContext,
    ) -> GraphResult<Self> {
        if !path.as_ref().is_dir() || std::fs::read_dir(path.as_ref())?.next().is_none() {
            return Err(invalid());
        }
        let store = Self::open_native(path.as_ref(), read_only, limits)?;
        store.inspect_import(context)?;
        Ok(store)
    }
    /// Recompute the persisted prefix; progress alone never proves fact effects.
    pub fn inspect_import(&self, context: &ProcessingContext) -> GraphResult<ImportProgress> {
        let _stage = profile::stage("import.inspect");
        context.check()?;
        self.with_read(|tx, options| {
            let r = progress(tx, options)?;
            let rows = FactRows::read(tx, options)?;
            let mutations = rows.mutations();
            let plan = Plan::new(
                &mutations,
                r.facts_digest.clone(),
                &ImportBudget {
                    max_batch_rows: r.max_batch_rows.try_into().map_err(|_| invalid())?,
                    max_batch_bytes: r.max_batch_bytes.try_into().map_err(|_| invalid())?,
                },
                context,
            )?;
            // Repartitioning a proper committed prefix must give the same chain.
            if r.committed_rows != mutations.len() as u64
                || r.completed_batches != plan.batches.len() as u64
                || r.prefix_digest != plan.at(plan.batches.len()).prefix_digest
            {
                return Err(invalid());
            }
            let metadata = MetadataRow::read(tx, options)?;
            if r.finalized {
                check_metadata(&metadata)?;
                let catalog = rows.decode(&metadata[0], context)?;
                if rows.digest()? != metadata[0].rows_digest
                    || stable_digest(&catalog)? != r.facts_digest
                    || metadata[0].content_digest != r.facts_digest
                    || r.plan_digest != plan.initial.plan_digest
                    || r.total_rows != r.committed_rows
                {
                    return Err(invalid());
                }
            } else if !metadata.is_empty() {
                return Err(invalid());
            }
            ImportProgress::try_from(&r)
        })
    }
    pub fn write_facts_batched(
        &self,
        catalog: &FactCatalog,
        budget: &ImportBudget,
        context: &ProcessingContext,
    ) -> GraphResult<ImportProgress> {
        self.write_facts_batched_observed(catalog, budget, context, |_| {})
    }
    /// Observer runs only after a new READ ONLY transaction confirms a batch;
    /// it may request cancellation for the next boundary, not revoke that batch.
    pub fn write_facts_batched_observed(
        &self,
        catalog: &FactCatalog,
        budget: &ImportBudget,
        context: &ProcessingContext,
        observed: impl FnMut(&ImportProgress),
    ) -> GraphResult<ImportProgress> {
        self.write_facts_batched_admitted(catalog, budget, context, || Ok(()), observed)
    }

    /// Admission is checked before the initial write, every new batch, and
    /// finalization. Refusal preserves confirmed prefixes, not a success root.
    pub fn write_facts_batched_admitted(
        &self,
        catalog: &FactCatalog,
        budget: &ImportBudget,
        context: &ProcessingContext,
        mut admit: impl FnMut() -> GraphResult<()>,
        mut observed: impl FnMut(&ImportProgress),
    ) -> GraphResult<ImportProgress> {
        let _stage = profile::stage("import.write");
        profile::measure("import.validate", || catalog.validate(context))?;
        let rows = profile::measure("import.encode", || FactRows::encode(catalog))?;
        if rows.cost() > self.limits.max_rows {
            return Err(GraphError::local(GraphErrorKind::ResourceExhausted));
        }
        let mutations = rows.mutations();
        let plan = profile::measure("import.plan", || {
            Plan::new(&mutations, stable_digest(catalog)?, budget, context)
        })?;
        admit()?;
        self.with_write(|tx, options| {
            let existing = ImportRow::read(tx, options)?;
            if existing.is_empty() {
                if FactRows::read(tx, options)?.cost() != 0
                    || !MetadataRow::read(tx, options)?.is_empty()
                {
                    return Err(conflict());
                }
                plan.initial.insert(tx, options)?;
            }
            Ok(())
        })?;
        let persisted = self.inspect_import(context)?;
        if persisted.plan_digest != plan.initial.plan_digest {
            return Err(conflict());
        }
        if persisted.finalized {
            return Ok(persisted);
        }
        for i in
            usize::try_from(persisted.completed_batches).map_err(|_| invalid())?..plan.batches.len()
        {
            context.check()?;
            admit()?;
            let batch = &plan.batches[i];
            let expected = plan.at(i);
            let next = plan.at(i + 1);
            profile::measure("import.batch_write", || {
                self.with_write(|tx, options| {
                    if progress(tx, options)? != expected {
                        return Err(conflict());
                    }
                    FactRows::import_batch(&mutations[batch.range.clone()], tx, options, context)?;
                    advance(tx, options, &next)
                })
            })?;
            profile::measure("import.batch_confirm", || {
                self.with_read(|tx, options| {
                    if progress(tx, options)? != next {
                        return Err(invalid());
                    }
                    Ok(())
                })
            })?;
            observed(&ImportProgress::try_from(&next)?);
        }
        context.check()?;
        self.with_read(|tx, options| {
            if FactRows::read(tx, options)? != rows {
                return Err(invalid());
            }
            Ok(())
        })?;
        let mut complete = plan.at(plan.batches.len());
        complete.finalized = true;
        admit()?;
        self.with_write(|tx, options| {
            if progress(tx, options)? != plan.at(plan.batches.len()) {
                return Err(conflict());
            }
            context.check()?;
            MetadataRow {
                id: "facts".into(),
                schema_version: schema::SCHEMA_VERSION,
                schema_fingerprint: schema::fingerprint()?,
                facts_format_version: crate::relations::bound::FACTS_FORMAT_VERSION,
                config_fingerprint: catalog.config_fingerprint.clone(),
                selection_fingerprint: catalog.selection_fingerprint.clone(),
                content_digest: plan.initial.facts_digest.clone(),
                rows_digest: rows.digest()?,
            }
            .insert(tx, options)?;
            advance(tx, options, &complete)
        })?;
        // No post-commit cancellation override of a finalized fact effect.
        self.inspect_import(&ProcessingContext {
            cancellation: tokio_util::sync::CancellationToken::default(),
            deadline: None,
            ..context.clone()
        })
        .map_err(|error| super::GraphError {
            kind: error.kind,
            outcome: Some(super::GraphOutcome::Committed),
            stage: "facts_confirm".into(),
            native_kind: None,
            transaction_code: None,
            mutation_code: None,
            cleanup: None,
            cause: Some(Box::new(error)),
        })
    }
}
