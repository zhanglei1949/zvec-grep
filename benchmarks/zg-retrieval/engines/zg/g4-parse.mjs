import assert from "node:assert/strict";
import { parseVisibleResponse } from "./parse.mjs";
import { sha256, objectHash } from "../../core/io.mjs";
import { coverage, scopeViolations } from "../../core/g4.mjs";

export function publicText(response) {
  assert.ok(
    response &&
      Array.isArray(response.content) &&
      response.content.length === 1 &&
      response.content[0].type === "text" &&
      typeof response.content[0].text === "string",
    "invalid public response",
  );
  return response.content[0].text;
}

export function parseGraphResponse(response, family) {
  const text = publicText(response);
  if (response.isError) return { status: "product_error", text };
  if (family === "explore") {
    const graph = JSON.parse(text);
    assert.ok(
      graph.view &&
        Array.isArray(graph.locations) &&
        Array.isArray(graph.evidence),
      "invalid explore output",
    );
    assert.ok(
      ["complete", "partial"].includes(graph.execution?.status),
      "missing execution status",
    );
    return { status: "success", text, graph, items: [] };
  }
  const marker = "\nGraph\n```json\n";
  const i = text.lastIndexOf(marker);
  let ordinary = text,
    graph = null;
  if (i >= 0) {
    assert.ok(text.endsWith("\n```"), "truncated graph JSON");
    ordinary = text.slice(0, i).trimEnd();
    graph = JSON.parse(text.slice(i + marker.length, -4));
    assert.ok(
      ["complete", "partial", "degraded"].includes(graph.status),
      "unknown graph status",
    );
  }
  const parsed = parseVisibleResponse({
    content: [{ type: "text", text: ordinary }],
  });
  return { ...parsed, ordinary_text: ordinary, graph, status: "success", text };
}

/** Only actual public source lines and explicit verified content earn evidence credit. */
export function visibleRanges(parsed, sources) {
  const visible = [];
  for (const item of parsed.items ?? []) {
    const source = sources.get(item.path);
    assert.ok(source, `unrecorded source ${item.path}`);
    const text = source.toString("utf8"),
      lines = text.split("\n");
    const starts = [0];
    for (let i = 0; i < lines.length - 1; i++)
      starts.push(starts.at(-1) + Buffer.byteLength(lines[i]) + 1);
    for (const [i, line] of item.source_lines.entries()) {
      const original = lines[line.line - 1]?.replace(/\r$/, "");
      assert.equal(
        typeof original,
        "string",
        `unknown frozen line ${item.path}:${line.line}`,
      );
      // A fragment may start after the indentation on its first source line.
      // Credit only the bytes actually printed, never the omitted prefix.
      const column = original.indexOf(line.text);
      assert.ok(
        column >= 0,
        `visible line differs from frozen ${item.path}:${line.line}`,
      );
      if (!line.text.length || column !== original.lastIndexOf(line.text))
        continue;
      const endColumn = column + line.text.length;
      const consecutive =
        endColumn === original.length &&
        item.source_lines[i + 1]?.line === line.line + 1;
      visible.push({
        path: item.path,
        sha256: sha256(source),
        start:
          starts[line.line - 1] + Buffer.byteLength(original.slice(0, column)),
        end:
          starts[line.line - 1] +
          Buffer.byteLength(original.slice(0, endColumn)) +
          (consecutive ? (lines[line.line - 1].endsWith("\r") ? 2 : 1) : 0),
        kind: "retrieved",
      });
    }
    assert.equal(
      item.unnumbered_source.length,
      0,
      "unlocatable public source cannot establish byte evidence",
    );
  }
  for (const read of parsed.graph?.evidence ?? []) {
    if (read.status === "retrieved") continue; // A pointer is not a second copy of visible text.
    assert.equal(read.status, "verified", "unverified graph evidence");
    const ref = read.reference,
      source = sources.get(ref.source_id);
    assert.ok(source, `unrecorded graph source ${ref.source_id}`);
    assert.equal(
      ref.raw_digest,
      sha256(source),
      "raw evidence digest mismatch",
    );
    assert.equal(
      ref.decoded_digest,
      sha256(source),
      "decoded evidence digest mismatch",
    );
    assert.ok(
      Number.isSafeInteger(ref.span.start) &&
        Number.isSafeInteger(ref.span.end) &&
        ref.span.start >= 0 &&
        ref.span.end >= ref.span.start &&
        ref.span.end <= source.length,
    );
    const content = source.subarray(ref.span.start, ref.span.end);
    assert.equal(
      content.toString("utf8"),
      read.content,
      "graph evidence bytes mismatch",
    );
    assert.equal(
      Buffer.byteLength(read.content),
      content.length,
      "non-UTF8 evidence boundary",
    );
    visible.push({
      path: ref.source_id,
      sha256: ref.raw_digest,
      ...ref.span,
      kind: "graph",
    });
  }
  return visible;
}

export function collectNodes(parsed, map = new Map()) {
  const graph = parsed.graph;
  if (!graph) return map;
  const nodes = [
    ...(graph.nodes ?? []),
    ...(graph.locations ?? []).flatMap((l) => l.candidates),
    ...(graph.scope?.anchor?.candidates ?? []),
  ];
  for (const n of nodes) {
    const previous = map.get(n.node.node_id);
    if (previous)
      assert.equal(
        objectHash({ ...previous, distance: null }),
        objectHash({ ...n, distance: null }),
        "node identity changed",
      );
    map.set(n.node.node_id, n);
  }
  return map;
}

function matchesNode(actual, expected) {
  return (
    actual?.source_id === expected.path &&
    actual.name === expected.name &&
    actual.span.start === expected.span.start &&
    actual.span.end === expected.span.end
  );
}

export function scoreExplore(task, parsed, sources, nodeMap) {
  if (parsed.status !== "success")
    return { passed: false, issues: ["product_error"] };
  const result = parsed.graph,
    issues = [];
  if (result.execution.status !== "complete")
    issues.push("incomplete_operation");
  if (
    JSON.stringify(result.locations.map((l) => l.status)) !==
    JSON.stringify(task.contract.locations)
  )
    issues.push("location_status");
  const nodes = [
    ...result.nodes,
    ...result.locations.flatMap((l) => l.candidates),
  ];
  const missingNodes = task.truth.nodes.filter(
    (n) => !nodes.some((a) => matchesNode(a, n)),
  );
  if (missingNodes.length) issues.push("missing_required_nodes");
  const references = [
    ...result.relations.map((r) => r.reference).filter(Boolean),
    ...result.references,
  ];
  const missedEdges = task.truth.edges.filter(
    (e) =>
      !references.some(
        (r) =>
          r.status === "resolved" &&
          r.kind === e.kind &&
          r.evidence.source_id === e.evidence.path &&
          r.evidence.span.start === e.evidence.start &&
          r.evidence.span.end === e.evidence.end &&
          matchesNode(nodeMap.get(r.target), e.target) &&
          matchesNode(nodeMap.get(r.owner), e.source),
      ),
  );
  if (missedEdges.length) issues.push("missing_required_edges");
  for (const site of task.contract.must_not_resolve ?? []) {
    const found = references.filter(
      (r) =>
        r.evidence.source_id === site.path &&
        r.evidence.span.start === site.start &&
        r.evidence.span.end === site.end,
    );
    if (
      !found.length ||
      found.some((r) => r.status === "resolved" || !r.reason)
    )
      issues.push("incorrect_dynamic_resolution");
  }
  if (Object.hasOwn(task.contract, "path_hops")) {
    const p = result.paths;
    if (task.contract.path_hops === null) {
      if (p.length) issues.push("unexpected_path");
    } else if (
      p.length !== 1 ||
      p[0].relations.length !== task.contract.path_hops ||
      JSON.stringify(
        p[0].nodes.map((id) => nodeMap.get(id.node_id ?? id)?.name),
      ) !== JSON.stringify(task.contract.path_names)
    )
      issues.push("wrong_path");
  }
  const visible = visibleRanges(parsed, sources);
  const edgesTotal = task.truth.edges.length;
  return {
    passed: !issues.length,
    issues,
    missing_nodes: missingNodes.map((n) => ({ path: n.path, name: n.name })),
    required_edges: edgesTotal,
    matched_required_edges: edgesTotal - missedEdges.length,
    edge_coverage: edgesTotal
      ? (edgesTotal - missedEdges.length) / edgesTotal
      : null,
    unjudged_references: Math.max(
      0,
      references.length - (edgesTotal - missedEdges.length),
    ),
    evidence: coverage(task.truth.required_evidence, visible),
    completion: result.execution.status,
  };
}

export function scoreSearch(task, call, parsed, sources) {
  if (parsed.status !== "success")
    return { status: "product_error", evidence: null };
  const visible = visibleRanges(parsed, sources);
  const required =
    call.family === "constrain"
      ? task.retrieval.scope_required_evidence
      : task.retrieval.required_evidence;
  const violations =
    call.arm === "constrain"
      ? scopeViolations(visible, task.retrieval.allowed_regions)
      : [];
  const scope = parsed.graph?.scope;
  const metadataIssues = [];
  if (call.arm === "constrain")
    for (const item of parsed.items) {
      const regions = task.retrieval.allowed_regions.filter(
        (r) => r.path === item.path,
      );
      if (!regions.length)
        metadataIssues.push({
          path: item.path,
          reason: "item_outside_allowed_sources",
        });
      for (const line of item.outline ?? [])
        if (line.trim() && !regions.some((r) => r.quote.includes(line.trim())))
          metadataIssues.push({
            path: item.path,
            reason: "outline_not_proven_in_scope",
          });
      for (const line of item.metadata ?? []) {
        const match = /^symbol: (?:[a-z_]+ )?([A-Za-z_][A-Za-z_0-9]*)/.exec(
          line,
        );
        if (
          match &&
          !regions.some((r) =>
            new RegExp(
              `\\b(?:fn|def|class|struct|trait|enum|type|const|static)\\s+${match[1]}\\b`,
            ).test(r.quote),
          )
        )
          metadataIssues.push({
            path: item.path,
            reason: "symbol_not_proven_in_scope",
            symbol: match[1],
          });
      }
    }
  return {
    status: "success",
    evidence: coverage(required, visible),
    output_bytes: Buffer.byteLength(parsed.text),
    graph_status: parsed.graph?.status ?? null,
    diagnostics: parsed.graph?.diagnostics ?? [],
    scope: scope ?? null,
    scope_outcome:
      call.arm !== "constrain"
        ? null
        : !scope?.complete
          ? "incomplete"
          : scope.empty_reason
            ? "empty_scope"
            : parsed.items.length
              ? "matches"
              : "no_matches_in_nonempty_scope",
    scope_violations: violations,
    scope_metadata_issues: metadataIssues,
    valid_scope:
      call.arm !== "constrain" ||
      Boolean(
        scope?.complete &&
        scope.exact_filter &&
        !violations.length &&
        !metadataIssues.length,
      ),
    ordinary_identity: sha256(parsed.ordinary_text),
    visible_ranges: visible,
  };
}

/** Precision only at independently labelled reference sites, never across unknown gold. */
export function judgeRelations(graph, nodeMap, truthEdges) {
  const references = [
    ...(graph?.relations ?? []).map((r) => r.reference).filter(Boolean),
    ...(graph?.references ?? []),
  ];
  const result = {
    known_correct: 0,
    known_wrong: 0,
    known_identity_unavailable: 0,
    unjudged: 0,
  };
  for (const ref of references) {
    const expected = truthEdges.filter(
      (e) =>
        e.kind === ref.kind &&
        e.evidence.path === ref.evidence.source_id &&
        e.evidence.start === ref.evidence.span.start &&
        e.evidence.end === ref.evidence.span.end,
    );
    if (!expected.length) {
      result.unjudged++;
      continue;
    }
    if (ref.status !== "resolved") continue;
    const target = nodeMap.get(ref.target);
    if (!target) {
      result.known_identity_unavailable++;
      continue;
    }
    if (expected.some((e) => matchesNode(target, e.target)))
      result.known_correct++;
    else result.known_wrong++;
  }
  result.precision_at_known_sites =
    result.known_correct + result.known_wrong
      ? result.known_correct / (result.known_correct + result.known_wrong)
      : null;
  return result;
}
