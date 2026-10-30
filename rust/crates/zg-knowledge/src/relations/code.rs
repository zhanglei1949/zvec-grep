mod lookup;
use super::{FileFacts, Language, Lookup, ResolvedReference, resolved};
use crate::{Error, Result, processing::ProcessingContext};
use lookup::{Found, Resolver, module_path};

/// Resolves supplied candidate facts only. No filesystem discovery or runtime inference.
pub fn resolve_code(
    files: &[FileFacts],
    context: &ProcessingContext,
) -> Result<Vec<ResolvedReference>> {
    let mut output = Vec::new();
    visit_code(files, context, |r| {
        output.push(r);
        Ok(())
    })?;
    Ok(output)
}

pub(crate) fn visit_code(
    files: &[FileFacts],
    context: &ProcessingContext,
    mut emit: impl FnMut(ResolvedReference) -> Result<()>,
) -> Result<()> {
    for file in files {
        file.validate_structure(context)?;
    }
    let mut output_cost = 0usize;
    let mut visits = 0usize;
    for file in files.iter().filter(|f| f.language != Language::Markdown) {
        let module = file
            .module
            .as_ref()
            .ok_or(Error::Provider("missing code module"))?;
        let catalog = files
            .iter()
            .filter(|f| {
                f.language == file.language
                    && f.module
                        .as_ref()
                        .is_some_and(|m| m.namespace == module.namespace)
            })
            .collect::<Vec<_>>();
        let dependencies = catalog
            .iter()
            .map(|f| f.version.clone())
            .collect::<Vec<_>>();
        let mut resolver = Resolver::new(&catalog, context, &mut visits);
        for reference in &file.references {
            context.check()?;
            let mut found = if file.complete {
                match &reference.lookup {
                    Lookup::Import { path } => resolver.path(
                        file,
                        reference.scope,
                        path,
                        &module_path(file, reference.scope),
                    )?,
                    Lookup::Name { parts } => resolver.name(file, reference, parts)?,
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
            output_cost = output_cost.saturating_add(1 + dependencies.len() + found.targets.len());
            context.check_output(output_cost)?;
            emit(resolved(
                file,
                reference,
                found.targets,
                found.status,
                found.reason,
                dependencies.clone(),
            ))?;
        }
    }
    Ok(())
}
