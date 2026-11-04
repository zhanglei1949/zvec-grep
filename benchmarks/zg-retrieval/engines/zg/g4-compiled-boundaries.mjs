import assert from "node:assert/strict";
import { mkdir, readFile, writeFile } from "node:fs/promises";
import { join, resolve } from "node:path";
import { platform } from "node:os";
import { parseArgs } from "node:util";
import { loadG4Suite } from "../../core/g4.mjs";
import {
  COMPILED_PROFILE,
  COMPILED_REVISION,
  compiledFixtures,
  compiledCorpusManifest,
  materializeCompiledCorpus,
  unpackCompiledSources,
  verifyCompiledModel,
} from "../../core/g4-compiled.mjs";
import {
  fileHash,
  objectHash,
  readJson,
  run,
  writeJson,
} from "../../core/io.mjs";
import { runCli } from "../../core/cli.mjs";
import { PublicMcp } from "./g4-rpc.mjs";
import { diskInventory, freePort } from "./g4-run.mjs";
import { parseGraphResponse, publicText, visibleRanges } from "./g4-parse.mjs";

export function verifyDefaultSchema(tools, protocol, budget) {
  const schema = tools.tools.find(
    (t) => t.name === "zvec_grep_explore",
  ).inputSchema;
  const execution = schema.$defs.ExecutionOptions.properties;
  assert.deepEqual(
    execution.budget.default,
    budget,
    "public default budget changed",
  );
  assert.deepEqual(
    Object.fromEntries(
      Object.entries(schema.$defs.BudgetLimits.properties).map(
        ([name, spec]) => [name, spec.default],
      ),
    ),
    budget,
  );
  for (const name of ["request_timeout_ms", "graph_timeout_ms"])
    assert.equal(
      execution[name].default,
      protocol.execution[name],
      "public default timeout changed",
    );
}

export function normalizeBoundaryGraph(graph) {
  assert.equal(graph.execution.status, "complete");
  const normalized = structuredClone(graph);
  delete normalized.execution.qualification_micros;
  delete normalized.execution.query_micros;
  delete normalized.execution.usage.output_bytes;
  return normalized;
}

export function boundaryReference(graph, boundary, inputs) {
  const sites = inputs.reference_annotations.filter(
    (r) =>
      r.repository === boundary.repository &&
      r.resolution_expectation === boundary.expected &&
      boundary.evidence.some(
        (e) =>
          e.path === r.site.path &&
          e.start <= r.site.start &&
          r.site.end <= e.end,
      ),
  );
  assert.equal(
    sites.length,
    1,
    "boundary needs one independently frozen callee site",
  );
  const { site, expression_context: context } = sites[0];
  const refs = graph.references.filter(
    (r) =>
      r.kind === "calls" &&
      r.evidence.source_id === site.path &&
      r.evidence.span.start <= site.start &&
      site.end <= r.evidence.span.end &&
      context.start <= r.evidence.span.start &&
      r.evidence.span.end <= context.end,
  );
  assert.equal(refs.length, 1, "missing or ambiguous boundary reference");
  const ref = refs[0];
  if (boundary.expected === "unresolved") {
    assert.equal(ref.status, "unresolved");
    assert.equal(ref.target, null, "unresolved reference invented a target");
  } else assert.ok(["unresolved", "resolved"].includes(ref.status));
  return ref;
}

async function capture(command, args, options) {
  try {
    return await run(command, args, options);
  } catch (error) {
    if (!error.result) throw error;
    return error.result;
  }
}

export async function main(args = process.argv.slice(2)) {
  const { values } = parseArgs({
    args,
    options: {
      output: { type: "string" },
      protocol: { type: "string", default: COMPILED_PROFILE },
      "binary-sha256": { type: "string" },
      "api-sha256": { type: "string" },
      binary: { type: "string" },
      "api-helper": { type: "string" },
      "native-library-dir": { type: "string" },
      "model-cache": { type: "string" },
    },
  });
  assert.ok(
    values.output &&
      values.binary &&
      values["api-helper"] &&
      values["native-library-dir"] &&
      values["model-cache"],
    "boundary runner needs all five paths",
  );
  const base = resolve(values.output),
    binary = resolve(values.binary),
    api = resolve(values["api-helper"]);
  await mkdir(base);
  const profile = values.protocol;
  const suite = await loadG4Suite(profile);
  assert.ok([COMPILED_PROFILE, COMPILED_REVISION].includes(profile));
  const handoff = await readJson(join(compiledFixtures, "handoff.json"));
  const freeze = await readJson(join(compiledFixtures, "freeze.json"));
  assert.equal(
    await fileHash(binary),
    profile === COMPILED_PROFILE
      ? handoff.binary.sha256
      : values["binary-sha256"],
    "boundary binary differs from pinned artifact",
  );
  if (profile === COMPILED_REVISION)
    assert.equal(await fileHash(api), values["api-sha256"], "API helper drift");
  const libraryDir = resolve(values["native-library-dir"]);
  const library = join(
    libraryDir,
    platform() === "darwin" ? "libzvec_c_api.dylib" : "libzvec_c_api.so",
  );
  assert.equal(await fileHash(library), handoff.native_library.sha256);
  const modelCache = resolve(values["model-cache"]);
  const modelBefore = await verifyCompiledModel(modelCache);
  const sources = await unpackCompiledSources(join(base, "frozen-sources"));
  await writeJson(join(base, "identity.json"), {
    suite: suite.identity,
    binary: { path: binary, sha256: await fileHash(binary) },
    api_helper: { path: api, sha256: await fileHash(api) },
    native_library_sha256: await fileHash(library),
    model: modelBefore,
    budget: freeze.budget,
    budget_overrides: false,
  });
  const results = [];
  for (const boundary of suite.inputs.boundary_cases) {
    const expected =
      suite.protocol.parser_conformance?.[boundary.case_id] ??
      boundary.expected;
    const directory = join(base, boundary.case_id.replace(":", "-"));
    await mkdir(directory);
    const root = join(directory, "workspace"),
      home = join(directory, "runtime");
    const repo = suite.inputs.repositories.find(
      (r) => r.repository === boundary.repository,
    );
    let client, env, before, corpusBefore;
    const record = {
      case_id: boundary.case_id,
      expected,
      frozen_v1_expected: boundary.expected,
      planned_entrypoints: ["rust_api", "cli_direct", "cli_daemon", "mcp"],
      outcomes: {},
      issues: [],
    };
    try {
      await materializeCompiledCorpus(repo, root, sources, boundary);
      corpusBefore = await compiledCorpusManifest(root, repo, boundary);
      await mkdir(home);
      const port = await freePort();
      await writeJson(join(home, "config.json"), {
        version: 1,
        server: { host: "127.0.0.1", port },
      });
      env = {
        ...process.env,
        ZVEC_GREP_HOME: home,
        ZVEC_GREP_WORKSPACE_REGISTRY: join(home, "registry.json"),
        ZVEC_GREP_AUTHORIZATION_KEY_FILE: join(home, "authorization.key"),
        ZVEC_GREP_MODEL_CACHE: modelCache,
        ZVEC_GREP_DEVICE: suite.protocol.device,
        NO_COLOR: "1",
        FORCE_COLOR: "0",
        [platform() === "darwin" ? "DYLD_LIBRARY_PATH" : "LD_LIBRARY_PATH"]:
          libraryDir,
      };
      delete env.ZG_GRAPH_PROFILE;
      const graphFile = join(directory, "graph.json");
      // Boundary workspaces have their own single language root; main-slice
      // mappings may mention directories absent from a boundary's frozen files.
      await writeJson(graphFile, {
        scope: "code",
        module_roots: [
          {
            root: "",
            language: boundary.language,
            namespace: `compiled-${boundary.language}`,
            module_prefix: [],
            layout: "flat",
          },
        ],
      });
      const indexed = await capture(
        binary,
        [
          "index",
          root,
          "--mode",
          "direct",
          "--embedding",
          suite.protocol.model,
          "--device",
          suite.protocol.device,
          "--model-cache",
          modelCache,
          "--graph-options",
          graphFile,
          "--glob",
          "!**/go.mod",
          "--glob",
          "!compile_commands.json",
        ],
        { env, cwd: root },
      );
      await writeJson(join(directory, "index.json"), indexed);
      assert.equal(indexed.code, 0, "boundary preparation failed");
      if (expected === "not_ready")
        assert.ok(indexed.stdout.includes("NotReady"));
      const input = join(directory, "request.json");
      await writeJson(input, boundary.explore_request);
      before = await diskInventory(join(root, ".zvec-grep"));
      await writeJson(join(directory, "storage-before.json"), before);
      client = new PublicMcp(
        [
          binary,
          "server",
          "--stdio",
          "--listen",
          `127.0.0.1:${port}`,
          "--mcp-toolset",
          "full",
        ],
        { env, cwd: root },
      );
      await writeJson(
        join(directory, "initialize.json"),
        await client.initialize(),
      );
      const tools = await client.request("tools/list", {});
      await writeJson(join(directory, "tools.json"), tools);
      verifyDefaultSchema(tools, suite.protocol, freeze.budget);
      const commands = {
        rust_api: [api, ["explore", root, input]],
        cli_direct: [
          binary,
          ["explore", "--root", root, "--mode", "direct", "--request", input],
        ],
        cli_daemon: [
          binary,
          ["explore", "--root", root, "--mode", "server", "--request", input],
        ],
      };
      for (const [name, [command, argv]] of Object.entries(commands)) {
        const raw = await capture(command, argv, { env, cwd: root });
        record.outcomes[name] = raw;
        await writeJson(join(directory, `${name}.json`), raw);
      }
      const raw = await client.request("tools/call", {
        name: "zvec_grep_explore",
        arguments: { root, request: boundary.explore_request },
      });
      record.outcomes.mcp = raw;
      await writeJson(join(directory, "mcp.json"), raw);
      if (expected === "not_ready") {
        for (const [name, response] of Object.entries(record.outcomes)) {
          const message =
            name === "mcp" ? publicText(response) : response.stderr;
          if (name === "mcp") assert.equal(response.isError, true);
          else assert.equal(response.code, 1);
          assert.ok(
            message.includes("code graph domain is not_ready"),
            `${name}: wrong not_ready error`,
          );
        }
        record.result = "not_ready";
      } else {
        const sourceMap = new Map(
          await Promise.all(
            boundary.workspace_files.map(async (path) => [
              path,
              await readFile(join(root, path)),
            ]),
          ),
        );
        const graphs = {};
        for (const [name, response] of Object.entries(record.outcomes)) {
          if (name !== "mcp") assert.equal(response.code, 0, `${name} failed`);
          const parsed = parseGraphResponse(
            name === "mcp"
              ? response
              : { content: [{ type: "text", text: response.stdout }] },
            "explore",
          );
          assert.equal(parsed.status, "success");
          visibleRanges(parsed, sourceMap, true);
          for (const [key, limit] of Object.entries(freeze.budget))
            assert.ok(
              parsed.graph.execution.usage[key] <= limit,
              `${name} exceeded default ${key}`,
            );
          graphs[name] = normalizeBoundaryGraph(parsed.graph);
        }
        for (const name of ["cli_direct", "cli_daemon", "mcp"])
          assert.deepEqual(
            graphs[name],
            graphs.rust_api,
            `${name} differs from Rust API`,
          );
        if (expected === "ready_locate") {
          const candidates = graphs.rust_api.locations.flatMap(
            (l) => l.candidates,
          );
          assert.equal(candidates.length, 1);
          const evidence = boundary.evidence[0];
          assert.equal(candidates[0].source_id, evidence.path);
          assert.ok(
            candidates[0].span.start <= evidence.start &&
              evidence.end <= candidates[0].span.end,
          );
          record.result = "ready_locate";
        } else {
          const reference = boundaryReference(
            graphs.rust_api,
            boundary,
            suite.inputs,
          );
          if (
            boundary.expected === "declaration_only_or_unresolved" &&
            reference.status === "resolved"
          ) {
            const request = {
              domain: "code",
              operation: {
                operation: "locate",
                target: {
                  by: "node",
                  node: {
                    graph_build_id: graphs.mcp.view.graph.build_id,
                    node_id: reference.target,
                  },
                },
              },
            };
            const identity = await client.request("tools/call", {
              name: "zvec_grep_explore",
              arguments: { root, request },
            });
            await writeJson(
              join(directory, "native-declaration-identity.json"),
              {
                request,
                response: identity,
              },
            );
            const parsed = parseGraphResponse(identity, "explore");
            assert.equal(parsed.status, "success");
            visibleRanges(parsed, sourceMap, true);
            const candidates = parsed.graph.locations[0].candidates;
            assert.equal(candidates.length, 1);
            const declared = boundary.evidence[1];
            assert.equal(candidates[0].source_id, declared.path);
            assert.deepEqual(candidates[0].span, {
              start: declared.start,
              end: declared.end,
            });
            assert.ok(
              declared.quote.includes(" native ") &&
                declared.quote.endsWith(";"),
            );
          }
          record.result = reference.status;
        }
        record.complete_and_valid_evidence = true;
      }
      record.default_schema_verified = true;
    } catch (error) {
      record.issues.push(error.message);
    } finally {
      if (client) {
        await client.close();
        await writeFile(join(directory, "mcp-stderr.log"), client.stderr);
      }
      if (env) {
        const shutdown = await capture(binary, ["server", "off"], {
          env,
          cwd: root,
          timeout: 30_000,
        });
        await writeJson(join(directory, "shutdown.json"), shutdown);
        if (shutdown.code !== 0) record.issues.push("daemon shutdown failed");
      }
      if (before) {
        const after = await diskInventory(join(root, ".zvec-grep"));
        await writeJson(join(directory, "storage-after.json"), after);
        record.storage_unchanged = objectHash(before) === objectHash(after);
        if (!record.storage_unchanged)
          record.issues.push("storage changed during boundary queries");
        const afterCorpus = await compiledCorpusManifest(root, repo, boundary);
        record.corpus_unchanged =
          objectHash(afterCorpus) === objectHash(corpusBefore);
        if (!record.corpus_unchanged)
          record.issues.push("frozen corpus changed");
      }
      record.executed_entrypoints = Object.keys(record.outcomes).length;
      record.passed =
        record.issues.length === 0 && record.executed_entrypoints === 4;
      await writeJson(join(directory, "result.json"), record);
      results.push(record);
      console.log(
        `${boundary.case_id}: ${record.passed ? "passed" : record.issues.join("; ")}`,
      );
    }
  }
  const modelAfter = await verifyCompiledModel(modelCache);
  assert.equal(
    modelAfter.sha256,
    modelBefore.sha256,
    "model changed during boundary checks",
  );
  await loadG4Suite(profile);
  const report = {
    scope: "R2 only; no 800-call retrieval execution",
    suite: suite.identity,
    planned_public_calls: 32,
    executed_public_calls: results.reduce(
      (n, r) => n + r.executed_entrypoints,
      0,
    ),
    passed: results.every((r) => r.passed),
    model_unchanged: true,
    frozen_inputs_unchanged: true,
    results,
  };
  await writeJson(join(base, "report.json"), report);
  if (!report.passed) process.exitCode = 1;
}

runCli(import.meta.url, () => main());
