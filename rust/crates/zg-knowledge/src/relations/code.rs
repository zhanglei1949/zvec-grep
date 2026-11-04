mod lookup;
use super::{FileFacts, Language, Lookup, ResolvedReference, SourceVersion, resolved};
use crate::{Error, Result, processing::ProcessingContext};
use lookup::{Found, Resolver, module_path};

/// Resolves supplied candidate facts only. No filesystem discovery or runtime inference.
pub fn resolve_code(
    files: &[FileFacts],
    context: &ProcessingContext,
) -> Result<Vec<ResolvedReference>> {
    let facts: Vec<&FileFacts> = files.iter().collect();
    let mut output = Vec::new();
    visit_code(&facts, context, |mut r, catalog| {
        r.dependencies = dependencies(catalog);
        output.push(r);
        Ok(())
    })?;
    Ok(output)
}

pub(crate) fn dependencies(catalog: &[&FileFacts]) -> Vec<SourceVersion> {
    catalog.iter().map(|f| f.version.clone()).collect()
}

/// Emits each resolution with the namespace members it was resolved against;
/// only callers that publish the G1 read set turn those into `dependencies`.
pub(crate) fn visit_code(
    files: &[&FileFacts],
    context: &ProcessingContext,
    mut emit: impl FnMut(ResolvedReference, &[&FileFacts]) -> Result<()>,
) -> Result<()> {
    for file in files {
        file.validate_structure(context)?;
    }
    let mut output_cost = 0usize;
    let mut visits = 0usize;
    for file in files
        .iter()
        .copied()
        .filter(|f| f.language != Language::Markdown)
    {
        let module = file
            .module
            .as_ref()
            .ok_or(Error::Provider("missing code module"))?;
        let catalog = files
            .iter()
            .copied()
            .filter(|f| {
                f.language == file.language
                    && f.module
                        .as_ref()
                        .is_some_and(|m| m.namespace == module.namespace)
            })
            .collect::<Vec<_>>();
        let mut resolver = Resolver::new(&catalog, context, &mut visits);
        for reference in &file.references {
            context.check()?;
            let mut relation_source = None;
            let mut found = if file.complete {
                match &reference.lookup {
                    Lookup::TypeRelation { owner, target } => {
                        let (found, source) =
                            resolver.type_relation(file, reference, owner, target)?;
                        relation_source = source;
                        found
                    }
                    Lookup::Import { path } => resolver.path(
                        file,
                        reference.scope,
                        path,
                        &module_path(file, reference.scope),
                    )?,
                    Lookup::Name { parts } => resolver.name(file, reference, parts)?,
                    Lookup::Include {
                        path,
                        system,
                        target,
                    } => resolver.include(file, path, *system, target.as_deref())?,
                    _ => Found::unknown("dynamic_target"),
                }
            } else {
                Found::unknown("incomplete_parse")
            };
            found
                .targets
                .sort_by(|a, b| (&a.path, &a.node_id).cmp(&(&b.path, &b.node_id)));
            found.targets.dedup();
            if !found.complete {
                found.targets.clear();
            }
            if reference.kind == super::RelationKind::Call
                && found.targets.iter().any(|t| {
                    catalog
                        .iter()
                        .find(|f| f.version.path == t.path)
                        .and_then(|f| f.nodes.iter().find(|n| n.id == t.node_id))
                        .is_none_or(|n| !n.callable)
                })
            {
                found = Found::unknown("target_not_static_function");
            }
            if !type_targets(file, reference, &catalog, &found.targets) {
                found = Found::unknown("target_not_type");
            }
            output_cost = output_cost.saturating_add(1 + catalog.len() + found.targets.len());
            context.check_output(output_cost)?;
            let mut resolution =
                resolved(file, reference, found.targets, found.status, found.reason);
            resolution.relation_source = relation_source;
            emit(resolution, &catalog)?;
        }
    }
    Ok(())
}

fn type_targets(
    file: &FileFacts,
    reference: &super::Reference,
    catalog: &[&FileFacts],
    targets: &[super::Target],
) -> bool {
    if !matches!(
        reference.kind,
        super::RelationKind::Extends | super::RelationKind::Implements
    ) {
        return true;
    }
    let owner = file
        .nodes
        .iter()
        .find(|n| n.id == reference.owner)
        .map(|n| n.entity_kind);
    targets.iter().all(|target| {
        catalog
            .iter()
            .find(|f| f.version.path == target.path)
            .and_then(|f| f.nodes.iter().find(|n| n.id == target.node_id))
            .is_some_and(|node| match file.language {
                Language::Rust => node.entity_kind == super::EntityKind::Interface,
                Language::Java if reference.kind == super::RelationKind::Implements => {
                    node.entity_kind == super::EntityKind::Interface
                }
                Language::Java => Some(node.entity_kind) == owner,
                Language::Cpp => {
                    node.entity_kind == super::EntityKind::Class && node.path_addressable
                }
                _ => node.entity_kind == super::EntityKind::Class,
            })
    })
}
