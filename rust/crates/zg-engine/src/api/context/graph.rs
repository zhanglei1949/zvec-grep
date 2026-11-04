//! Canonical KG query types; host availability remains an engine policy.
pub use zg_knowledge::query::{
    BodyRegion, Direction, EntityKind, GraphQueryMode, Locator, NodeRef, QueryDomain, RelationKind,
    ScopeSpec, SourceFilter, TraversalSpec,
};

pub(crate) fn ensure_supported(mode: &GraphQueryMode, rg: bool) -> crate::EngineResult<()> {
    mode.validate()?;
    if !mode.is_off() {
        if rg {
            return Err(crate::EngineError::invalid_argument(
                "rg supports only graph mode off",
            ));
        }
        if !cfg!(feature = "knowledge-graph-graphdb") {
            return Err(crate::EngineError::unsupported(
                "graph augmentation requires the graphdb backend",
            ));
        }
        let expansion = match mode {
            GraphQueryMode::Augment { expansion, .. } => expansion,
            GraphQueryMode::Constrain { scope } => &scope.traversal,
            GraphQueryMode::Off {} => return Ok(()),
        };
        if expansion.relations.iter().any(|r| {
            !matches!(
                r,
                RelationKind::Contains
                    | RelationKind::Calls
                    | RelationKind::Imports
                    | RelationKind::Extends
                    | RelationKind::Implements
            )
        }) {
            return Err(crate::EngineError::unsupported(
                "code graph relation unsupported",
            ));
        }
    }
    Ok(())
}
