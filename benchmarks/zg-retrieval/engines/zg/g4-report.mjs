import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { join, resolve } from "node:path";
import {
  loadG4Suite,
  g4CallPlan,
  g4Request,
  evidenceContribution,
  mean,
  percentile,
} from "../../core/g4.mjs";
import { readJson, writeJson, objectHash, fileHash } from "../../core/io.mjs";
import { repositorySlug } from "../../core/corpus.mjs";
import {
  parseGraphResponse,
  collectNodes,
  scoreExplore,
  scoreSearch,
  judgeRelations,
} from "./g4-parse.mjs";
import { scoreFileRetrieval } from "../../metrics/files.mjs";
import { scoreNdcg } from "../../metrics/ndcg.mjs";

const key = (c) => JSON.stringify([c.task_id, c.family, c.arm, c.repetition]);
export async function aggregateG4(directory) {
  const suite = await loadG4Suite();
  const saved = await readJson(join(directory, "suite.json"));
  assert.equal(
    objectHash(saved.identity),
    objectHash(suite.identity),
    "frozen suite changed",
  );
  assert.equal(objectHash(saved.inputs), objectHash(suite.inputs));
  assert.equal(objectHash(saved.protocol), objectHash(suite.protocol));
  const repositories = [];
  for (const repo of suite.inputs.repositories) {
    const output = join(directory, repositorySlug(repo.repository));
    const manifest = await readJson(join(output, "run.json"));
    assert.equal(manifest.commit, repo.commit);
    assert.equal(objectHash(manifest.suite), objectHash(suite.identity));
    const tasks = suite.tasks.filter((t) => t.repository === repo.repository);
    const expected = g4CallPlan(tasks, suite.protocol),
      records = await readJson(join(output, "calls.json"));
    assert.equal(
      records.length,
      expected.length,
      "missing call matrix entries",
    );
    const issues = [...manifest.issues];
    if (manifest.status !== "ready" || !manifest.integrity_verified)
      issues.push("repository_preparation_or_integrity_failed");
    const sources = new Map();
    if (manifest.root)
      try {
        for (const f of await readJson(join(output, "sources.json"))) {
          const path = join(output, "sources", f.path);
          assert.equal(
            await fileHash(path),
            f.sha256,
            `source changed ${f.path}`,
          );
          sources.set(f.path, await readFile(path));
        }
      } catch (e) {
        issues.push(e.message);
      }
    if (manifest.integrity_verified) {
      for (const name of ["storage", "model", "corpus"]) {
        const before = await readJson(join(output, `${name}-before.json`)),
          after = await readJson(join(output, `${name}-after.json`));
        if (objectHash(before) !== objectHash(after))
          issues.push(`${name}_snapshot_changed`);
      }
    }
    const parsed = new Map(),
      nodeMap = new Map(),
      rows = [];
    for (const [i, c] of records.entries()) {
      assert.equal(key(c), key(expected[i]), "altered call order");
      const task = tasks.find((t) => t.task_id === c.task_id);
      assert.equal(
        objectHash(c.request),
        objectHash(
          g4Request(task, c, manifest.root ?? "<unavailable>", suite.protocol),
        ),
        "request differs from frozen plan",
      );
      if (c.status === "blocked") {
        rows.push({ ...c, execution_status: "blocked" });
        continue;
      }
      assert.equal(
        await fileHash(join(output, c.raw_path)),
        c.raw_sha256,
        "raw response changed",
      );
      assert.ok(
        Number.isFinite(c.latency_ms) && c.latency_ms >= 0,
        "missing latency",
      );
      const raw = await readJson(join(output, c.raw_path));
      try {
        const result = parseGraphResponse(raw, c.family);
        if (result.status === "success" && c.arm === "off")
          assert.equal(result.graph, null, "Off returned graph enhancements");
        if (result.status === "success" && c.arm !== "off")
          assert.ok(result.graph, "missing explicit graph status");
        parsed.set(key(c), result);
        if (
          result.graph?.view &&
          objectHash(result.graph.view) !== objectHash(manifest.view)
        )
          issues.push(`view_changed:${key(c)}`);
        if (c.arm === "explore" || c.arm !== "off")
          collectNodes(result, nodeMap);
      } catch (e) {
        parsed.delete(key(c));
        rows.push({
          ...c,
          execution_status: "invalid_response",
          error: e.message,
        });
        issues.push(`invalid_response:${key(c)}:${e.message}`);
      }
    }
    try {
      for (const read of await readJson(
        join(output, "reference-identities.json"),
      )) {
        assert.equal(read.request.name, "zvec_grep_explore");
        assert.equal(
          read.request.arguments.request.operation.operation,
          "locate",
        );
        assert.equal(
          read.request.arguments.request.operation.target.by,
          "node",
        );
        assert.equal(objectHash(read.response), read.response_sha256);
        const result = parseGraphResponse(read.response, "explore");
        assert.equal(objectHash(result.graph.view), objectHash(manifest.view));
        assert.equal(result.graph.locations[0].status, "unique");
        assert.equal(
          result.graph.locations[0].candidates[0].node.node_id,
          read.request.arguments.request.operation.target.node.node_id,
        );
        collectNodes(result, nodeMap);
      }
    } catch (e) {
      if (e.code !== "ENOENT") issues.push(`reference_identity:${e.message}`);
    }
    for (const c of records) {
      const result = parsed.get(key(c));
      if (!result) continue;
      const task = tasks.find((t) => t.task_id === c.task_id);
      const row = { ...c, execution_status: result.status };
      if (task.annotation?.status === "pending") {
        row.execution_status = "pending_gold";
        rows.push(row);
        continue;
      }
      row.relations = judgeRelations(
        result.graph,
        nodeMap,
        tasks.flatMap((t) => t.truth?.edges ?? []),
      );
      try {
        if (c.family === "explore")
          row.score = scoreExplore(task, result, sources, nodeMap);
        else {
          row.score = scoreSearch(task, c, result, sources);
          if (result.status === "success") {
            const targetPaths =
              c.family === "constrain"
                ? [
                    ...new Set(
                      task.retrieval.allowed_regions.map((r) => r.path),
                    ),
                  ]
                : task.retrieval.target_files;
            const targets = targetPaths.map((path) => ({
              path,
            }));
            row.file_retrieval = scoreFileRetrieval(result.items, targets);
            row.ndcg = scoreNdcg(result.items, targets);
          }
          if (
            !row.score.valid_scope &&
            c.arm === "constrain" &&
            result.status === "success"
          )
            issues.push(`invalid_scope:${key(c)}`);
        }
      } catch (e) {
        row.execution_status = "invalid_evidence";
        row.error = e.message;
        issues.push(`invalid_evidence:${key(c)}:${e.message}`);
      }
      if (c.transport_error || result.status === "product_error")
        issues.push(`runtime_error:${key(c)}`);
      rows.push(row);
    }
    assert.equal(
      rows.length,
      records.length,
      "one row per planned call required",
    );
    const pairs = [];
    for (const c of records.filter(
      (c) => c.arm !== "off" && c.arm !== "explore",
    )) {
      const off = records.find(
        (x) =>
          x.task_id === c.task_id &&
          x.family === c.family &&
          x.arm === "off" &&
          x.repetition === c.repetition,
      );
      const a = parsed.get(key(off)),
        b = parsed.get(key(c));
      const successful = a?.status === "success" && b?.status === "success";
      const same =
        c.family === "augment" && successful
          ? a.ordinary_text === b.ordinary_text
          : null;
      if (same === false) issues.push(`ordinary_results_changed:${key(c)}`);
      const baseline = rows.find((x) => key(x) === key(off)),
        treatment = rows.find((x) => key(x) === key(c));
      const usable =
        successful &&
        baseline.execution_status === "success" &&
        treatment.execution_status === "success";
      const contribution = usable
        ? evidenceContribution(
            c.family === "constrain"
              ? tasks.find((t) => t.task_id === c.task_id).retrieval
                  .scope_required_evidence
              : tasks.find((t) => t.task_id === c.task_id).retrieval
                  .required_evidence,
            baseline.score.visible_ranges,
            treatment.score.visible_ranges,
          )
        : null;
      pairs.push({
        contribution,
        task_id: c.task_id,
        family: c.family,
        repetition: c.repetition,
        quality_observation: c.quality_observation,
        legacy: Boolean(tasks.find((t) => t.task_id === c.task_id)?.legacy),
        usable,
        ordinary_unchanged: same,
        latency_delta_ms: usable ? c.latency_ms - off.latency_ms : null,
        evidence_byte_fraction_delta:
          usable &&
          treatment.score.evidence.byte_fraction !== null &&
          baseline.score.evidence.byte_fraction !== null
            ? treatment.score.evidence.byte_fraction -
              baseline.score.evidence.byte_fraction
            : null,
        evidence_delta:
          usable &&
          treatment.score?.evidence?.complete_fraction !== null &&
          baseline.score?.evidence?.complete_fraction !== null
            ? treatment.score.evidence.complete_fraction -
              baseline.score.evidence.complete_fraction
            : null,
      });
    }
    const summary = {
      repository: repo.repository,
      commit: repo.commit,
      preparation_status: manifest.status,
      candidate: manifest.candidate ?? null,
      view: manifest.view ?? null,
      planned_calls: expected.length,
      executed_calls: records.filter((r) => r.status !== "blocked").length,
      blocked_calls: records.filter((r) => r.status === "blocked").length,
      integrity_passed: issues.length === 0,
      issues: [...new Set(issues)],
      explore: {
        total: tasks.filter((t) => t.explore_request).length,
        passed: rows.filter((r) => r.family === "explore" && r.score?.passed)
          .length,
      },
      cost: {
        text_index: manifest.text_index,
        graph_index: manifest.graph_index,
        mcp_index_ms: manifest.mcp_index_ms,
        text_bytes: manifest.text_bytes,
        graph_bytes: manifest.graph_bytes,
      },
      quality: [],
      pairs,
      rows,
    };
    for (const family of ["augment", "constrain"]) {
      const quality = pairs.filter(
        (p) => p.family === family && p.quality_observation && !p.legacy,
      );
      const selectedRows = rows.filter(
        (r) =>
          r.family === family &&
          r.quality_observation &&
          !tasks.find((t) => t.task_id === r.task_id)?.legacy,
      );
      summary.quality.push({
        family,
        tasks: quality.length,
        usable_pairs: quality.filter((p) => p.usable).length,
        mean_evidence_byte_fraction_delta: mean(
          quality
            .filter((p) => p.usable)
            .map((p) => p.evidence_byte_fraction_delta)
            .filter((v) => v !== null),
        ),
        mean_additional_bytes: mean(
          quality
            .filter((p) => p.usable)
            .map((p) => p.contribution.additional_bytes),
        ),
        mean_known_required_additional_bytes: mean(
          quality
            .filter((p) => p.usable)
            .map((p) => p.contribution.known_required_additional_bytes),
        ),
        mean_unjudged_additional_bytes: mean(
          quality
            .filter((p) => p.usable)
            .map((p) => p.contribution.unjudged_additional_bytes),
        ),
        mean_evidence_delta: mean(
          quality
            .filter((p) => p.usable)
            .map((p) => p.evidence_delta)
            .filter((x) => x !== null),
        ),
        latency_delta_p50_ms: percentile(
          pairs
            .filter((p) => p.family === family && p.usable && !p.legacy)
            .map((p) => p.latency_delta_ms),
          0.5,
        ),
        latency_delta_p95_ms: percentile(
          pairs
            .filter((p) => p.family === family && p.usable && !p.legacy)
            .map((p) => p.latency_delta_ms),
          0.95,
        ),
        degraded_calls: rows.filter(
          (r) =>
            r.family === family &&
            r.arm === family &&
            !tasks.find((t) => t.task_id === r.task_id)?.legacy &&
            r.score?.graph_status === "degraded",
        ).length,
        scope_violation_calls: rows.filter(
          (r) => r.family === family && r.score?.scope_violations?.length,
        ).length,
        arms: ["off", family].map((arm) => {
          const all = rows.filter(
            (r) =>
              r.family === family &&
              r.arm === arm &&
              !tasks.find((t) => t.task_id === r.task_id)?.legacy,
          );
          const q = selectedRows.filter(
            (r) => r.arm === arm && r.execution_status === "success",
          );
          return {
            arm,
            quality_samples: q.length,
            errors: all.filter((r) => r.execution_status !== "success").length,
            evidence_coverage: mean(
              q
                .map((r) => r.score.evidence?.complete_fraction)
                .filter((n) => n !== null),
            ),
            evidence_byte_fraction: mean(
              q
                .map((r) => r.score.evidence?.byte_fraction)
                .filter((v) => v !== null),
            ),
            file_mrr_at_10: mean(q.map((r) => r.file_retrieval.rr_at_10)),
            file_hit_at_1: mean(q.map((r) => r.file_retrieval.hit_at_1)),
            file_hit_at_5: mean(q.map((r) => r.file_retrieval.hit_at_5)),
            file_hit_at_10: mean(q.map((r) => r.file_retrieval.hit_at_10)),
            ndcg_at_10: mean(q.map((r) => r.ndcg.ndcg_at_10)),
            output_bytes: mean(q.map((r) => r.score.output_bytes)),
            latency_p50_ms: percentile(
              all
                .filter((r) => r.execution_status === "success")
                .map((r) => r.latency_ms),
              0.5,
            ),
            latency_p95_ms: percentile(
              all
                .filter((r) => r.execution_status === "success")
                .map((r) => r.latency_ms),
              0.95,
            ),
          };
        }),
      });
    }
    summary.legacy_rows = rows.filter(
      (r) => tasks.find((t) => t.task_id === r.task_id)?.legacy,
    );
    summary.correctness_passed =
      summary.explore.passed === summary.explore.total;
    summary.relations_at_known_sites = {
      known_correct: 0,
      known_wrong: 0,
      known_identity_unavailable: 0,
      unjudged: 0,
    };
    for (const row of rows.filter((r) => r.quality_observation))
      for (const k of Object.keys(summary.relations_at_known_sites))
        summary.relations_at_known_sites[k] += row.relations?.[k] ?? 0;
    repositories.push(summary);
  }
  const report = {
    schema_version: 7,
    protocol: suite.protocol.id,
    identity: suite.identity,
    scope: suite.protocol.scope,
    integrity_passed: repositories.every((r) => r.integrity_passed),
    correctness_passed: repositories.every((r) => r.correctness_passed),
    repositories,
    limits: [
      "Two public repositories and source-reviewed development questions; no human quality review.",
      "Partial independent source oracle: unlabelled relations are unknown; no global precision claim.",
      "Only public visible text earns evidence credit; graph pointers are not extra content.",
      "Original build path is used with executable/runtime SHA-256 equal to the package payload; extracted-package startup remains unvalidated on this macOS host.",
      "Five frozen contract annotations have review issues: four untyped function/file name collisions and a parameter declaration mistaken for a call site. Labels remain unchanged; these are not established product defects.",
      "Shared-session timings are not cold-disk or statistical SLA claims; errors retained.",
      "requests:16 is a separate legacy row; not a rerun of the full SWE-QA20.",
    ],
  };
  await writeJson(join(directory, "report.json"), report);
  let md = `# G4 代码图评测\n\n协议：${report.protocol}。运行完整性：${report.integrity_passed ? "通过" : "未全部通过，失败保留"}。\n\n`;
  md +=
    "| 仓库 | 执行／计划调用 | explore 通过 | 完整性 |\n| --- | --- | --- | --- |\n";
  for (const r of repositories)
    md += `| ${r.repository} | ${r.executed_calls}/${r.planned_calls} | ${r.explore.passed}/${r.explore.total} | ${r.integrity_passed ? "通过" : "失败／阻塞"} |\n`;
  md +=
    "\n## 检索对照\n\n| 仓库 | 对照 | 有效质量配对 | 证据完整覆盖率平均变化 | 附加耗时 P50（ms） |\n| --- | --- | --- | --- | --- |\n";
  for (const r of repositories)
    for (const q of r.quality)
      md += `| ${r.repository} | Off / ${q.family} | ${q.usable_pairs}/${q.tasks} | ${q.mean_evidence_delta ?? "不可用"} | ${q.latency_delta_p50_ms ?? "不可用"} |\n`;
  md +=
    "\n完整性失败时，以上仅作逐题诊断，不构成正式效果结论。失败调用、每题评分、额外关系未知量、原始响应摘要和构建成本见 report.json。旧 requests 题单列在 rows 中。\n\n## 边界\n\n" +
    report.limits.map((s) => `- ${s}`).join("\n") +
    "\n";
  await writeJson(join(directory, "verification.json"), {
    schema_version: 1,
    integrity_passed: report.integrity_passed,
    report_sha256: await fileHash(join(directory, "report.json")),
    identity: suite.identity,
  });
  const { writeFile } = await import("node:fs/promises");
  await writeFile(join(directory, "report.md"), md);
  return report;
}
export async function main(args = process.argv.slice(2)) {
  assert.equal(args.length, 1, "G4 replay needs one run directory");
  const result = await aggregateG4(resolve(args[0]));
  if (!result.integrity_passed) process.exitCode = 1;
}
