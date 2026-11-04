import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { join } from "node:path";
import { suiteDirectory } from "./suite.mjs";
import { readJson, objectHash, sha256 } from "./io.mjs";
import {
  COMPILED_PROFILES,
  COMPILED_REVISION,
  compiledFixtures,
  verifyCompiledFreeze,
} from "./g4-compiled.mjs";

export const G4_PROFILE = "g4-code-v7";
export const G4_PROFILES = [
  G4_PROFILE,
  "g4-code-v8",
  "g4-code-v9",
  ...COMPILED_PROFILES,
];

export async function loadG4Suite(profile = G4_PROFILE) {
  assert.ok(G4_PROFILES.includes(profile), "unknown G4 profile");
  const protocol = await readJson(
    join(suiteDirectory, `configs/${profile}.json`),
  );
  assert.equal(protocol.schema_version, 7);
  assert.equal(protocol.repetitions, 5);
  assert.equal(protocol.quality_repetition, 5);
  const inputs = await readJson(join(suiteDirectory, protocol.inputs));
  const compiled = COMPILED_PROFILES.includes(profile);
  if (compiled) {
    await verifyCompiledFreeze(profile);
    assert.equal(
      inputs.id,
      profile === COMPILED_REVISION
        ? "g4-compiled-source-v2"
        : "g4-compiled-source-v1",
    );
    if (profile === COMPILED_REVISION) {
      const original = await readJson(
        join(suiteDirectory, "data/g4-compiled-v1.json"),
      );
      const expected = structuredClone(original);
      expected.id = "g4-compiled-source-v2";
      const java = expected.repositories.find((r) => r.language === "java");
      java.selection.graph.module_roots = [
        { ...java.selection.graph.module_roots[0], root: "tools/java_api/src" },
      ];
      assert.deepEqual(
        inputs,
        expected,
        "revision changed frozen questions, answers or source",
      );
      const previous = await readJson(
        join(suiteDirectory, "configs/g4-compiled-v1.json"),
      );
      assert.deepEqual(
        protocol,
        {
          ...previous,
          id: "zg-code-graph-four-languages-v2",
          inputs: "data/g4-compiled-v2.json",
          parser_conformance: { "cpp:B-02": "ready_locate" },
        },
        "revision changed model, scoring or budgets",
      );
    }
    assert.equal(inputs.boundary_cases.length, 8);
    assert.ok(inputs.tasks.every((t) => t.case_role === "normal"));
    const spec = await readFile(
      join(suiteDirectory, "gold/g4-compiled-v1-spec.json"),
    );
    assert.equal(sha256(spec), inputs.annotation_spec_sha256);
  } else if (profile !== G4_PROFILE) {
    assert.equal(inputs.id, "g4-code-source-v2");
    assert.equal(
      inputs.tasks.filter((t) => t.case_role === "normal").length,
      43,
    );
    assert.equal(
      inputs.tasks.filter((t) => t.case_role === "budget_boundary").length,
      5,
    );
    assert.ok(
      inputs.tasks
        .filter((t) => t.case_role === "budget_boundary")
        .every((t) => t.budget_expectation),
    );
  }
  if (profile === "g4-code-v9" || compiled)
    assert.equal(protocol.evidence_scoring, "visible-bytes-v2");
  assert.equal(inputs.tasks.length, 48);
  assert.equal(inputs.repositories.length, compiled ? 4 : 2);
  assert.equal(
    inputs.tasks.filter((t) => t.retrieval).length,
    compiled ? 48 : 24,
  );
  assert.equal(
    inputs.tasks.filter((t) => t.retrieval?.constrain).length,
    compiled ? 32 : 16,
  );
  assert.equal(new Set(inputs.tasks.map((t) => t.task_id)).size, 48);
  for (const repo of inputs.repositories) {
    assert.match(repo.commit, /^[a-f0-9]{40}$/);
    assert.equal(
      inputs.tasks.filter((t) => t.repository === repo.repository).length,
      compiled ? 12 : 24,
    );
  }
  for (const t of inputs.tasks) {
    assert.equal(t.annotation.human_reviewed, false);
    assert.ok(["source_reviewed", "pending"].includes(t.annotation.status));
    assert.ok(inputs.repositories.some((r) => r.repository === t.repository));
    if (t.retrieval)
      assert.equal(sha256(t.retrieval.query), t.retrieval.query_sha256);
    for (const e of t.truth.required_evidence) validateRange(e);
    if (compiled && t.retrieval.constrain) {
      for (const e of t.retrieval.allowed_regions) validateRange(e);
      for (const e of t.retrieval.scope_required_evidence) validateRange(e);
    }
  }
  const tool = await readFile(
    join(
      suiteDirectory,
      compiled
        ? "tools/g4-compiled-labels.py"
        : profile === G4_PROFILE
          ? "tools/g4-labels.py"
          : "tools/g4-labels-v2.py",
    ),
  );
  assert.equal(
    sha256(tool),
    inputs.annotation_tool_sha256,
    "annotation generator changed after freeze",
  );
  if (compiled) {
    const frozen = await readJson(join(compiledFixtures, "call-plan.json"));
    const calls = g4CallPlan(inputs.tasks, protocol).map((call) => {
      const task = inputs.tasks.find((t) => t.task_id === call.task_id);
      return {
        ...call,
        query_sha256: task.retrieval.query_sha256,
        normalized_request_sha256: sha256(
          JSON.stringify(
            g4Request(
              task,
              call,
              `/frozen-source/${task.repository.replaceAll("/", "__")}`,
              protocol,
            ),
          ),
        ),
      };
    });
    assert.deepEqual(calls, frozen.calls, "frozen 800-call matrix changed");
    return {
      protocol,
      inputs,
      tasks: inputs.tasks,
      identity: { protocol: objectHash(protocol), inputs: objectHash(inputs) },
    };
  }
  // The original question and file labels remain byte-for-byte independent of G4.
  const legacy = await readJson(join(suiteDirectory, "data/source.lock.json"));
  const legacyGold = await readJson(join(suiteDirectory, "gold/files-v1.json"));
  const original = legacy.tasks.find((t) => t.task_id === "requests:16");
  const legacyTask = {
    ...original,
    language: "python",
    category: "legacy_file_retrieval",
    legacy: true,
    retrieval: {
      query: original.query,
      augment: { mode: "augment", seeds: 5, expansion: { max_hops: 2 } },
      target_files: legacyGold.tasks[original.task_id].targets.map(
        (t) => t.path,
      ),
      required_evidence: [],
    },
  };
  const tasks = [...inputs.tasks, legacyTask];
  return {
    protocol,
    inputs,
    tasks,
    identity: {
      protocol: objectHash(protocol),
      inputs: objectHash(inputs),
      legacy_task: objectHash(original),
      legacy_gold: objectHash(legacyGold.tasks[original.task_id]),
    },
  };
}

export function validateRange(e) {
  assert.ok(
    typeof e.path === "string" &&
      e.path &&
      !e.path.startsWith("/") &&
      !e.path.includes("\\") &&
      !e.path.split("/").some((p) => !p || p === ".." || p === "."),
    "invalid evidence path",
  );
  assert.match(e.sha256, /^[a-f0-9]{64}$/);
  assert.ok(
    Number.isSafeInteger(e.start) &&
      Number.isSafeInteger(e.end) &&
      e.start >= 0 &&
      e.end > e.start,
  );
  assert.equal(Buffer.byteLength(e.quote), e.end - e.start);
}

export function g4CallPlan(tasks, protocol) {
  const calls = [];
  for (const [i, task] of tasks.entries()) {
    if (task.explore_request)
      calls.push({
        task_id: task.task_id,
        family: "explore",
        arm: "explore",
        repetition: 1,
        quality_observation: true,
      });
    if (!task.retrieval) continue;
    for (const family of [
      "augment",
      ...(task.retrieval.constrain ? ["constrain"] : []),
    ]) {
      for (
        let repetition = 1;
        repetition <= protocol.repetitions;
        repetition++
      ) {
        const arms = (i + repetition) % 2 ? ["off", family] : [family, "off"];
        for (const arm of arms)
          calls.push({
            task_id: task.task_id,
            family,
            arm,
            repetition,
            quality_observation: repetition === protocol.quality_repetition,
          });
      }
    }
  }
  return calls;
}

export function g4Request(task, call, root, protocol) {
  if (call.family === "explore")
    return {
      name: "zvec_grep_explore",
      arguments: {
        root,
        request: task.explore_request,
        execution: protocol.execution,
      },
    };
  return {
    name: "zvec_grep_search",
    arguments: {
      root,
      query: task.retrieval.query,
      limit: protocol.limit,
      ...protocol.request,
      ...(call.arm === "off"
        ? {}
        : {
            graph: task.retrieval[call.arm],
            graphExecution: protocol.execution,
          }),
    },
  };
}

export function unionIntervals(intervals) {
  const result = [];
  for (const [a, b] of intervals
    .filter(([a, b]) => b > a)
    .sort((x, y) => x[0] - y[0] || x[1] - y[1])) {
    const previous = result.at(-1);
    if (previous && a <= previous[1]) previous[1] = Math.max(previous[1], b);
    else result.push([a, b]);
  }
  return result;
}

export function coverage(required, visible) {
  let bytes = 0,
    coveredBytes = 0,
    complete = 0;
  const unique = [
    ...new Map(
      required.map((e) => [
        JSON.stringify([e.path, e.sha256, e.start, e.end]),
        e,
      ]),
    ).values(),
  ];
  for (const e of unique) {
    const matching = unionIntervals(
      visible
        .filter((v) => v.path === e.path && v.sha256 === e.sha256)
        .map((v) => [Math.max(e.start, v.start), Math.min(e.end, v.end)]),
    );
    const covered = matching.reduce((n, [a, b]) => n + b - a, 0);
    bytes += e.end - e.start;
    coveredBytes += covered;
    complete += Number(covered === e.end - e.start);
  }
  return {
    required: unique.length,
    complete,
    complete_fraction: unique.length ? complete / unique.length : null,
    required_bytes: bytes,
    covered_bytes: coveredBytes,
    byte_fraction: bytes ? coveredBytes / bytes : null,
  };
}

export function scopeViolations(visible, allowed) {
  return visible.filter((v) => {
    const match = unionIntervals(
      allowed
        .filter((a) => a.path === v.path && a.sha256 === v.sha256)
        .map((a) => [a.start, a.end]),
    );
    return !match.some(([a, b]) => a <= v.start && b >= v.end);
  });
}

export function mean(values) {
  return values.length
    ? values.reduce((a, b) => a + b, 0) / values.length
    : null;
}
export function percentile(values, p) {
  return values.length
    ? [...values].sort((a, b) => a - b)[Math.ceil(p * values.length) - 1]
    : null;
}

/** Newly delivered bytes; partial gold leaves the remaining bytes unjudged. */
export function evidenceContribution(required, baseline, treatment) {
  const groups = new Map();
  for (const r of treatment) {
    const key = JSON.stringify([r.path, r.sha256]);
    if (!groups.has(key))
      groups.set(key, { path: r.path, sha256: r.sha256, intervals: [] });
    groups.get(key).intervals.push([r.start, r.end]);
  }
  let added = 0,
    known = 0;
  for (const { path, sha256: hash, intervals } of groups.values()) {
    const before = unionIntervals(
      baseline
        .filter((r) => r.path === path && r.sha256 === hash)
        .map((r) => [r.start, r.end]),
    );
    const gold = unionIntervals(
      required
        .filter((r) => r.path === path && r.sha256 === hash)
        .map((r) => [r.start, r.end]),
    );
    for (const [a, b] of unionIntervals(intervals)) {
      let pieces = [[a, b]];
      for (const [x, y] of before)
        pieces = pieces.flatMap(([p, q]) =>
          y <= p || x >= q
            ? [[p, q]]
            : [
                [p, Math.min(q, x)],
                [Math.max(p, y), q],
              ].filter(([s, e]) => e > s),
        );
      for (const [p, q] of pieces) {
        added += q - p;
        known += gold.reduce(
          (n, [x, y]) => n + Math.max(0, Math.min(q, y) - Math.max(p, x)),
          0,
        );
      }
    }
  }
  return {
    additional_bytes: added,
    known_required_additional_bytes: known,
    unjudged_additional_bytes: added - known,
  };
}
