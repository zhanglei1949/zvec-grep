use super::{
    QueryDomain, QueryError, QueryErrorKind, QueryResult, RelationKind, RequestContext, nonempty,
    source_id,
};
use serde::{Deserialize, Serialize};

/// Serializable identity only. This is not a read lease or proof of readiness.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ViewStamp {
    pub workspace_id: String,
    pub generation: String,
    pub text_revision: u64,
    pub graph: Option<GraphStamp>,
}
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct GraphStamp {
    pub build_id: String,
    pub schema: String,
    pub rules: String,
    pub configuration: String,
    pub catalog_revision: String,
}
impl ViewStamp {
    pub fn validate(&self) -> QueryResult<()> {
        nonempty(&self.workspace_id)?;
        nonempty(&self.generation)?;
        if let Some(graph) = &self.graph {
            for value in [
                &graph.build_id,
                &graph.schema,
                &graph.rules,
                &graph.configuration,
                &graph.catalog_revision,
            ] {
                nonempty(value)?;
            }
        }
        Ok(())
    }
    pub fn require_same(&self, other: &Self) -> QueryResult<()> {
        self.validate()?;
        other.validate()?;
        if self != other {
            return Err(mismatch());
        }
        Ok(())
    }
    pub fn require_graph(&self) -> QueryResult<&GraphStamp> {
        self.validate()?;
        self.graph.as_ref().ok_or_else(|| {
            QueryError::new(
                QueryErrorKind::GraphUnavailable,
                "view",
                "graph required for query inputs",
            )
        })
    }
    pub fn check_node(&self, node: &NodeRef) -> QueryResult<()> {
        self.validate()?;
        node.validate()?;
        if self
            .graph
            .as_ref()
            .is_none_or(|g| g.build_id != node.graph_build_id)
        {
            return Err(mismatch());
        }
        Ok(())
    }
}
fn mismatch() -> QueryError {
    QueryError::new(
        QueryErrorKind::ViewMismatch,
        "view",
        "query inputs belong to different views",
    )
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct NodeRef {
    pub graph_build_id: String,
    pub node_id: String,
}
impl NodeRef {
    pub fn validate(&self) -> QueryResult<()> {
        nonempty(&self.graph_build_id)?;
        nonempty(&self.node_id)
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GraphRequirement {
    None,
    Optional,
    Required,
}
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ReadViewRequest {
    pub graph: GraphRequirement,
    /// Required for Optional/Required, absent for a text-only request.
    pub needs: Option<GraphReadNeeds>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct GraphReadNeeds {
    pub domain: QueryDomain,
    pub relations: Vec<RelationKind>,
    pub text_mapping: bool,
}
impl GraphReadNeeds {
    pub fn validate(&self) -> QueryResult<()> {
        if self.relations.len() > 6
            || self
                .relations
                .iter()
                .enumerate()
                .any(|(i, relation)| self.relations[..i].contains(relation))
        {
            return Err(QueryError::invalid("invalid required graph relations"));
        }
        Ok(())
    }
}
impl ReadViewRequest {
    pub fn validate(&self) -> QueryResult<()> {
        match (self.graph, &self.needs) {
            (GraphRequirement::None, None) => Ok(()),
            (GraphRequirement::Optional | GraphRequirement::Required, Some(needs)) => {
                needs.validate()
            }
            _ => Err(QueryError::invalid(
                "graph requirement and domain needs disagree",
            )),
        }
    }
}

/// Provider-attested capabilities of ready domains in this selected view.
/// This is not a serialized client assertion or a promise about other builds.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct GraphReadCapabilities {
    pub domain: QueryDomain,
    pub relations: Vec<RelationKind>,
    pub text_mapping: bool,
}
impl GraphReadCapabilities {
    #[must_use]
    pub fn satisfies(&self, needs: &GraphReadNeeds) -> bool {
        self.domain == needs.domain
            && (!needs.text_mapping || self.text_mapping)
            && needs.relations.iter().all(|r| self.relations.contains(r))
    }
}

/// Retains host-owned resources without exposing native graph connections.
/// Only the host provider can attest metadata/lease compatibility; callers
/// cannot deserialize this from a client-supplied stamp.
#[derive(Debug)]
pub struct KgReadView<L> {
    stamp: ViewStamp,
    lease: L,
    capabilities: Vec<GraphReadCapabilities>,
    unavailable_reason: Option<String>,
}
impl<L> KgReadView<L> {
    pub fn new(
        stamp: ViewStamp,
        lease: L,
        request: &ReadViewRequest,
        capabilities: Vec<GraphReadCapabilities>,
        unavailable_reason: Option<String>,
    ) -> QueryResult<Self> {
        request.validate()?;
        stamp.validate()?;
        let has_graph = stamp.graph.is_some();
        if unavailable_reason
            .as_ref()
            .is_some_and(|s| s.trim().is_empty())
        {
            return Err(QueryError::invalid("empty graph unavailability reason"));
        }
        match request.graph {
            GraphRequirement::None if has_graph || unavailable_reason.is_some() => {
                return Err(QueryError::invalid("text-only view must not open a graph"));
            }
            GraphRequirement::Required if !has_graph => {
                return Err(QueryError::new(
                    QueryErrorKind::GraphUnavailable,
                    "open_view",
                    "required compatible graph unavailable",
                ));
            }
            GraphRequirement::Optional if !has_graph && unavailable_reason.is_none() => {
                return Err(QueryError::invalid(
                    "optional graph requires an unavailable reason",
                ));
            }
            _ => {}
        }
        if has_graph && unavailable_reason.is_some() {
            return Err(QueryError::invalid(
                "available graph cannot have an unavailable reason",
            ));
        }
        if !has_graph && !capabilities.is_empty() {
            return Err(QueryError::invalid(
                "text view cannot attest graph capabilities",
            ));
        }
        for (i, capability) in capabilities.iter().enumerate() {
            GraphReadNeeds {
                domain: capability.domain,
                relations: capability.relations.clone(),
                text_mapping: capability.text_mapping,
            }
            .validate()?;
            if capabilities[..i]
                .iter()
                .any(|c| c.domain == capability.domain)
            {
                return Err(QueryError::invalid("duplicate graph domain capability"));
            }
        }
        if has_graph
            && request.needs.as_ref().is_none_or(|needs| {
                !capabilities
                    .iter()
                    .any(|capability| capability.satisfies(needs))
            })
        {
            return Err(QueryError::new(
                QueryErrorKind::GraphUnavailable,
                "open_view",
                "selected graph does not satisfy domain or capability requirements",
            ));
        }
        Ok(Self {
            stamp,
            lease,
            capabilities,
            unavailable_reason,
        })
    }
    #[must_use]
    pub fn stamp(&self) -> &ViewStamp {
        &self.stamp
    }
    #[must_use]
    pub fn lease(&self) -> &L {
        &self.lease
    }
    #[must_use]
    pub fn unavailable_reason(&self) -> Option<&str> {
        self.unavailable_reason.as_deref()
    }
    pub fn require(&self, needs: &GraphReadNeeds) -> QueryResult<()> {
        needs.validate()?;
        if self
            .capabilities
            .iter()
            .any(|capability| capability.satisfies(needs))
        {
            Ok(())
        } else {
            Err(QueryError::new(
                QueryErrorKind::GraphUnavailable,
                "view",
                "requested graph domain or capability unavailable",
            ))
        }
    }
}

pub trait ReadViewProvider {
    type Lease: Send + Sync;
    /// Must retain read admission and attest version/domain compatibility. Only
    /// ready domains may be advertised; Optional unmet needs return a text view
    /// with a reason. A stamp
    /// alone is insufficient; opening never builds, repairs or publishes data.
    fn open_view(
        &self,
        request: &ReadViewRequest,
        context: &RequestContext,
    ) -> QueryResult<KgReadView<Self::Lease>>;
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ByteSpan {
    pub start: u64,
    pub end: u64,
}
impl ByteSpan {
    pub fn validate(self) -> QueryResult<()> {
        if self.start > self.end {
            return Err(QueryError::invalid("reversed UTF-8 byte span"));
        }
        Ok(())
    }
    /// Validate and slice the same decoded text, including UTF-8 boundaries.
    pub fn slice(self, text: &str) -> QueryResult<&str> {
        self.validate()?;
        let start =
            usize::try_from(self.start).map_err(|_| QueryError::invalid("span overflow"))?;
        let end = usize::try_from(self.end).map_err(|_| QueryError::invalid("span overflow"))?;
        text.get(start..end)
            .ok_or_else(|| QueryError::invalid("span outside decoded UTF-8 text"))
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct EvidenceRef {
    pub source_id: String,
    pub raw_digest: String,
    pub decoded_digest: String,
    pub span: ByteSpan,
    pub reference_id: Option<String>,
}
impl EvidenceRef {
    pub fn validate(&self) -> QueryResult<()> {
        source_id(&self.source_id)?;
        self.span.validate()?;
        for digest in [&self.raw_digest, &self.decoded_digest] {
            if digest.len() != 64 || !digest.bytes().all(|b| b.is_ascii_hexdigit()) {
                return Err(QueryError::invalid("invalid evidence digest"));
            }
        }
        if let Some(id) = &self.reference_id {
            nonempty(id)?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RankedHit {
    pub source_id: String,
    pub entity_id: String,
    pub fragment_id: String,
    pub source_spans: Vec<ByteSpan>,
    pub priority: u32,
    pub query_sources: Vec<String>,
}
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RankedHits {
    pub view: ViewStamp,
    pub hits: Vec<RankedHit>,
}
impl RankedHits {
    pub fn validate_for(&self, view: &ViewStamp) -> QueryResult<()> {
        view.require_same(&self.view)?;
        view.require_graph()?;
        for hit in &self.hits {
            source_id(&hit.source_id)?;
            nonempty(&hit.entity_id)?;
            nonempty(&hit.fragment_id)?;
            if hit.source_spans.is_empty() {
                return Err(QueryError::invalid(
                    "ranked hit requires mapped source spans",
                ));
            }
            for span in &hit.source_spans {
                span.validate()?;
                if span.start == span.end {
                    return Err(QueryError::invalid(
                        "ranked hit source spans must contain text",
                    ));
                }
            }
            if hit.source_spans.windows(2).any(|w| w[0].end > w[1].start) {
                return Err(QueryError::invalid(
                    "ranked hit source spans must be ordered and disjoint",
                ));
            }
            if hit.priority == 0 || hit.query_sources.is_empty() {
                return Err(QueryError::invalid(
                    "ranked hit requires priority and query provenance",
                ));
            }
            for source in &hit.query_sources {
                nonempty(source)?;
            }
        }
        Ok(())
    }
}

/// Scope creation belongs to the KG resolver, not serde or application input.
/// It also cannot be deserialized from a client payload:
/// ```compile_fail
/// use zg_knowledge::query::ResolvedScope;
/// let _: ResolvedScope = serde_json::from_str("{}").unwrap();
/// ```
/// Only the resolver can create a scope after complete traversal and mapping.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ResolvedScope {
    pub(super) view: ViewStamp,
    pub(super) regions: Vec<super::ScopeRegion>,
    pub(super) units: Vec<super::ScopeUnit>,
}
impl ResolvedScope {
    pub fn validate_for(&self, view: &ViewStamp) -> QueryResult<()> {
        view.require_same(&self.view)?;
        view.require_graph()?;
        Ok(())
    }
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.units.is_empty()
    }
    #[must_use]
    pub fn units(&self) -> &[super::ScopeUnit] {
        &self.units
    }
    #[must_use]
    pub fn regions(&self) -> &[super::ScopeRegion] {
        &self.regions
    }
    #[must_use]
    pub fn view(&self) -> &ViewStamp {
        &self.view
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum EvidenceRead {
    Verified {
        reference: EvidenceRef,
        content: String,
    },
    Unavailable {
        reference: EvidenceRef,
        reason: String,
    },
}
pub trait EvidenceProvider<L> {
    /// Read, hash, decode and slice one immutable input per source. Results
    /// preserve request order and report missing evidence individually.
    /// Implementations must check the supplied view against their read lease.
    fn read_evidence(
        &self,
        view: &KgReadView<L>,
        references: &[EvidenceRef],
        context: &RequestContext,
    ) -> QueryResult<Vec<EvidenceRead>>;
}

/// A contiguous, byte-preserving segment; removed/concatenated text uses
/// multiple entries. `entity_id` is the host retrieval entity, not a business
/// `NodeId`. Fragment ranges are joined from host metadata, not copied per map.
/// The containing build/source carries version and digest identity. This is a
/// query DTO, not a G2 persistence format.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TextMap {
    pub source_id: String,
    pub source_span: ByteSpan,
    pub entity_id: String,
    pub content_span: ByteSpan,
}
impl TextMap {
    pub fn validate(&self) -> QueryResult<()> {
        source_id(&self.source_id)?;
        nonempty(&self.entity_id)?;
        self.source_span.validate()?;
        self.content_span.validate()?;
        if self.source_span.start == self.source_span.end {
            return Err(QueryError::invalid("TextMap segments must not be empty"));
        }
        if self.source_span.end - self.source_span.start
            != self.content_span.end - self.content_span.start
        {
            return Err(QueryError::invalid(
                "TextMap segment is not byte-preserving",
            ));
        }
        Ok(())
    }
    pub fn validate_text(&self, source: &str, content: &str) -> QueryResult<()> {
        self.validate()?;
        if self.source_span.slice(source)? != self.content_span.slice(content)? {
            return Err(QueryError::invalid(
                "TextMap content differs from its source",
            ));
        }
        Ok(())
    }
}
