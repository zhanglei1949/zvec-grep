import assert from "node:assert/strict";
import {
  mkdir,
  readFile,
  writeFile,
  readdir,
  copyFile,
  stat,
} from "node:fs/promises";
import { join, resolve, dirname, delimiter } from "node:path";
import { parseArgs } from "node:util";
import { performance } from "node:perf_hooks";
import { cpus, platform, arch } from "node:os";
import { createServer } from "node:net";
import { loadG4Suite, g4CallPlan, g4Request } from "../../core/g4.mjs";
import {
  writeJson,
  readJson,
  run,
  fileHash,
  objectHash,
  inside,
} from "../../core/io.mjs";
import {
  corpusManifest,
  modelArtifactManifest,
  prepareCorpus,
  repositorySlug,
} from "../../core/corpus.mjs";
import { packageCandidate } from "./run.mjs";
import { PublicMcp } from "./g4-rpc.mjs";
import { aggregateG4 } from "./g4-report.mjs";
import {
  parseGraphResponse,
  visibleRanges,
  collectNodes,
} from "./g4-parse.mjs";

async function freePort() {
  const s = createServer();
  await new Promise((r, j) => {
    s.once("error", j);
    s.listen(0, "127.0.0.1", r);
  });
  const port = s.address().port;
  await new Promise((r) => s.close(r));
  return port;
}
export async function diskInventory(root, prefix = "") {
  const files = [];
  for (const e of (
    await readdir(join(root, prefix), { withFileTypes: true })
  ).sort((a, b) => a.name.localeCompare(b.name))) {
    const path = join(prefix, e.name);
    if (e.isDirectory()) files.push(...(await diskInventory(root, path)));
    else if (e.isFile())
      files.push({
        path,
        bytes: (await stat(join(root, path))).size,
        sha256: await fileHash(join(root, path)),
      });
    else throw new Error(`unexpected non-file in storage ${path}`);
  }
  return files;
}
async function cloneCorpus(repo, options) {
  if (options.sourceCache) {
    const source = join(options.sourceCache, repositorySlug(repo.repository));
    assert.equal(
      (await run("git", ["-C", source, "rev-parse", "HEAD"])).stdout.trim(),
      repo.commit,
    );
    const root = join(options.corpus, repositorySlug(repo.repository));
    await run("git", [
      "-c",
      "core.hooksPath=/dev/null",
      "clone",
      "--no-hardlinks",
      "--quiet",
      source,
      root,
    ]);
  }
  return prepareCorpus(repo, options.corpus);
}
async function measured(cli, args, output, env, cwd) {
  const start = performance.now();
  let result;
  const timerArgs = platform() === "darwin" ? ["-lp"] : ["-v"];
  const libraryEnv =
    platform() === "darwin" ? "DYLD_LIBRARY_PATH" : "LD_LIBRARY_PATH";
  const invocation = env[libraryEnv]
    ? ["/usr/bin/env", `${libraryEnv}=${env[libraryEnv]}`, cli, ...args]
    : [cli, ...args];
  try {
    result = await run("/usr/bin/time", [...timerArgs, ...invocation], {
      env,
      cwd,
      timeout: 2_400_000,
    });
  } catch (e) {
    result = e.result ?? { code: null, stderr: e.message, stdout: "" };
  }
  const rss =
    platform() === "darwin"
      ? /(\d+)\s+maximum resident set size/.exec(result.stderr)
      : /Maximum resident set size \(kbytes\):\s*(\d+)/.exec(result.stderr);
  const value = {
    ...result,
    wall_ms: performance.now() - start,
    peak_rss_bytes: rss
      ? Number(rss[1]) * (platform() === "darwin" ? 1 : 1024)
      : null,
    command: ["/usr/bin/time", ...timerArgs, ...invocation],
  };
  await writeJson(output, value);
  return value;
}

async function runRepo(suite, repo, candidate, options) {
  const output = join(options.output, repositorySlug(repo.repository));
  await mkdir(join(output, "raw"), { recursive: true });
  const tasks = suite.tasks.filter((t) => t.repository === repo.repository);
  const plan = g4CallPlan(tasks, suite.protocol);
  const manifest = {
    schema_version: 7,
    repository: repo.repository,
    commit: repo.commit,
    suite: suite.identity,
    candidate: candidate.identity,
    candidate_commit: options.candidateCommit,
    environment: { platform: platform(), arch: arch(), cpu: cpus()[0]?.model },
    protocol: suite.protocol,
    status: "preparing",
    planned_calls: plan.length,
    issues: [],
    started_at: new Date().toISOString(),
  };
  let root, env, client, before, modelBefore, corpusBefore;
  const records = [];
  try {
    root = await cloneCorpus(repo, options);
    manifest.root = root;
    assert.ok(
      !inside(root, options.output) && !inside(root, options.modelCache),
      "labels/artifacts/model cache inside indexed tree",
    );
    try {
      await stat(join(root, ".zvec-grep"));
      throw new Error("fresh checkout already has an index");
    } catch (e) {
      if (e.code !== "ENOENT") throw e;
    }
    corpusBefore = await corpusManifest(root);
    await writeJson(join(output, "corpus-before.json"), corpusBefore);
    const code = corpusBefore.entries.filter(
      (x) => x.kind === "file" && /\.(rs|py|pyi|pyw)$/.test(x.path),
    );
    for (const f of code) {
      const destination = join(output, "sources", f.path);
      await mkdir(resolve(destination, ".."), { recursive: true });
      await copyFile(join(root, f.path), destination);
    }
    await writeJson(join(output, "sources.json"), code);
    for (const t of tasks.filter((t) => t.explore_request))
      for (const e of t.truth.required_evidence) {
        const source = await readFile(join(output, "sources", e.path));
        assert.equal(
          await fileHash(join(output, "sources", e.path)),
          e.sha256,
          "stale label source",
        );
        assert.equal(
          source.subarray(e.start, e.end).toString(),
          e.quote,
          "stale label bytes",
        );
      }
    const home = join(output, "runtime-home"),
      opencode = join(output, "opencode.json");
    await mkdir(home, { recursive: true });
    await writeJson(join(home, ".zvec-grep/config.json"), {
      version: 1,
      server: { host: "127.0.0.1", port: await freePort() },
      defaults: {
        embedding: suite.protocol.model,
        modelCacheDir: options.modelCache,
      },
      models: { [suite.protocol.model]: { device: suite.protocol.device } },
    });
    env = {
      ...process.env,
      HOME: home,
      USERPROFILE: home,
      OPENCODE_CONFIG: opencode,
      ZVEC_GREP_HOME: join(home, ".zvec-grep"),
      ZVEC_GREP_WORKSPACE_REGISTRY: join(home, "registry.json"),
      ZVEC_GREP_AUTHORIZATION_KEY_FILE: join(home, "authorization.key"),
      ZVEC_GREP_MODEL_CACHE: options.modelCache,
      ZVEC_GREP_DEVICE: "cpu",
      NO_COLOR: "1",
      FORCE_COLOR: "0",
      npm_config_cache: join(options.output, "npm-cache"),
      PATH: [
        dirname(candidate.cli),
        join(candidate.consumer, "node_modules/.bin"),
        process.env.PATH ?? "",
      ].join(delimiter),
    };
    delete env.ZG_GRAPH_PROFILE;
    env[platform() === "darwin" ? "DYLD_LIBRARY_PATH" : "LD_LIBRARY_PATH"] =
      options.nativeLibraryDir;
    await writeJson(
      join(output, "install.json"),
      await run(
        candidate.cli,
        [
          "install",
          "--target",
          "opencode",
          "--yes",
          "--mcp-transport",
          "stdio",
          "--mcp-toolset",
          "full",
        ],
        { env, cwd: root },
      ),
    );
    const config = await readJson(opencode);
    const command = config.mcp.zvec_grep.command;
    manifest.mcp_command = command;
    await run(candidate.cli, ["server", "off"], { env, cwd: root });
    const select = repo.selection;
    const args = [
      "index",
      root,
      "--mode",
      "direct",
      "--embedding",
      suite.protocol.model,
      "--model-cache",
      options.modelCache,
      "--device",
      "cpu",
      "--max-filesize",
      String(select.max_file_size_bytes),
    ];
    for (const ext of select.extensions) args.push("--iglob", `*${ext}`);
    console.log(`${repo.repository}: fresh text index`);
    manifest.text_index = await measured(
      candidate.cli,
      args,
      join(output, "text-index.json"),
      env,
      root,
    );
    assert.equal(
      manifest.text_index.code,
      0,
      "text index failed (including measurement failure)",
    );
    // Separate public CLI graph-only build provides exact process peak RSS.
    const graphFile = join(output, "graph-options.json");
    await writeJson(graphFile, select.graph);
    const textManifest = await readJson(join(root, ".zvec-grep/manifest.json"));
    const generation = join(
      root,
      ".zvec-grep/generations",
      textManifest.storageGeneration,
    );
    const textBefore = await diskInventory(join(generation, "storage"));
    console.log(`${repo.repository}: graph-only publication`);
    manifest.graph_index = await measured(
      candidate.cli,
      ["index", root, "--mode", "direct", "--graph-options", graphFile],
      join(output, "graph-index.json"),
      env,
      root,
    );
    assert.equal(manifest.graph_index.code, 0, "graph build failed");
    assert.equal(
      objectHash(await diskInventory(join(generation, "storage"))),
      objectHash(textBefore),
      "graph-only changed text/vector bytes",
    );
    manifest.graph_only_text_unchanged = true;
    const reopen = await run(
      candidate.cli,
      [
        "explore",
        "--mode",
        "direct",
        "locate",
        "--name",
        "__g4_readiness_probe__",
      ],
      { env, cwd: root },
    );
    await writeJson(join(output, "reopen.json"), reopen);
    const view = JSON.parse(reopen.stdout);
    assert.ok(view.coverage.ready && view.execution.status === "complete");
    manifest.view = view.view;
    client = new PublicMcp(command, { env, cwd: root });
    await writeJson(join(output, "initialize.json"), await client.initialize());
    const tools = await client.request("tools/list", {});
    await writeJson(join(output, "tools.json"), tools);
    for (const name of [
      "zvec_grep_search",
      "zvec_grep_explore",
      "zvec_grep_index",
    ])
      assert.ok(
        tools.tools.some((t) => t.name === name),
        `missing ${name}`,
      );
    const indexTool = tools.tools.find((t) => t.name === "zvec_grep_index");
    assert.ok(
      indexTool.inputSchema.properties.graph,
      "MCP index lacks graph configuration",
    );
    const indexStart = performance.now();
    const indexReply = await client.request(
      "tools/call",
      {
        name: "zvec_grep_index",
        arguments: { root, graph: select.graph, wait: true },
      },
      2_400_000,
    );
    manifest.mcp_index_ms = performance.now() - indexStart;
    await writeJson(join(output, "mcp-index.json"), indexReply);
    assert.ok(!indexReply.isError, "MCP graph index failed");
    const probe = await client.request("tools/call", {
      name: "zvec_grep_explore",
      arguments: {
        root,
        request: {
          domain: "code",
          operation: {
            operation: "locate",
            target: { by: "name", name: "__g4_readiness_probe__" },
          },
        },
      },
    });
    await writeJson(join(output, "mcp-reopen.json"), probe);
    assert.ok(!probe.isError, "MCP graph reopen failed");
    const publicProbe = JSON.parse(probe.content[0].text);
    assert.ok(
      publicProbe.coverage.ready && publicProbe.execution.status === "complete",
    );
    manifest.view = publicProbe.view;
    before = await diskInventory(join(root, ".zvec-grep"));
    await writeJson(join(output, "storage-before.json"), before);
    manifest.text_bytes = textBefore.reduce((n, f) => n + f.bytes, 0);
    manifest.graph_bytes = (
      await diskInventory(join(generation, "graph"))
    ).reduce((n, f) => n + f.bytes, 0);
    modelBefore = await modelArtifactManifest(options.modelCache);
    await writeJson(join(output, "model-before.json"), modelBefore);
    manifest.status = "ready";
    await writeJson(join(output, "run.json"), manifest);
    // Fixed smoke tasks are executed and retained separately before the matrix.
    const smoke = plan.filter(
      (c) =>
        [tasks[0].task_id, tasks[8].task_id].includes(c.task_id) &&
        c.repetition === 1,
    );
    for (const [i, c] of smoke.entries()) {
      const request = g4Request(
        tasks.find((t) => t.task_id === c.task_id),
        c,
        root,
        suite.protocol,
      );
      let response;
      try {
        response = await client.request("tools/call", request);
      } catch (e) {
        response = {
          isError: true,
          content: [{ type: "text", text: e.message }],
        };
      }
      await writeJson(join(output, `smoke-${i}.json`), {
        call: c,
        request,
        response,
      });
      const parsed = parseGraphResponse(response, c.family);
      if (parsed.status === "success") {
        const sources = new Map(
          await Promise.all(
            code.map(async (f) => [
              f.path,
              await readFile(join(output, "sources", f.path)),
            ]),
          ),
        );
        visibleRanges(parsed, sources);
      }
    }
    for (const [i, c] of plan.entries()) {
      const request = g4Request(
        tasks.find((t) => t.task_id === c.task_id),
        c,
        root,
        suite.protocol,
      );
      const start = performance.now();
      let response,
        error = null;
      try {
        response = await client.request("tools/call", request);
      } catch (e) {
        error = e.message;
        response = { isError: true, content: [{ type: "text", text: error }] };
      }
      const elapsed = performance.now() - start;
      const raw = `raw/${String(i).padStart(4, "0")}.json`;
      await writeJson(join(output, raw), response);
      records.push({
        ...c,
        request,
        latency_ms: elapsed,
        transport_error: error,
        raw_path: raw,
        raw_sha256: await fileHash(join(output, raw)),
      });
      await writeJson(join(output, "calls.json"), records);
      if (i % 20 === 0)
        console.log(`${repo.repository}: ${i + 1}/${plan.length} calls`);
    }
    await collectReferenceIdentities(
      client,
      output,
      root,
      tasks,
      records,
      manifest.view,
    );
  } catch (e) {
    manifest.status =
      manifest.status === "ready" ? "execution_failed" : "preparation_failed";
    manifest.issues.push(e.message);
    console.error(`${repo.repository}: ${e.message}`);
  } finally {
    if (client) {
      await client.close();
      await writeFile(join(output, "mcp-stderr.log"), client.stderr);
    }
    if (env)
      try {
        await run(candidate.cli, ["server", "off"], {
          env,
          cwd: root,
          timeout: 60_000,
        });
      } catch (e) {
        manifest.issues.push(`shutdown: ${e.message}`);
      }
    if (before)
      try {
        const after = await diskInventory(join(root, ".zvec-grep"));
        await writeJson(join(output, "storage-after.json"), after);
        assert.equal(
          objectHash(before),
          objectHash(after),
          "storage changed during queries",
        );
        const afterModel = await modelArtifactManifest(options.modelCache);
        await writeJson(join(output, "model-after.json"), afterModel);
        assert.equal(afterModel.sha256, modelBefore.sha256, "model changed");
        const corpusAfter = await corpusManifest(root);
        await writeJson(join(output, "corpus-after.json"), corpusAfter);
        assert.equal(corpusAfter.sha256, corpusBefore.sha256, "corpus changed");
        manifest.integrity_verified = true;
      } catch (e) {
        manifest.issues.push(e.message);
        manifest.integrity_verified = false;
      }
    for (const c of plan.slice(records.length))
      records.push({
        ...c,
        status: "blocked",
        reason: manifest.issues.join("; "),
        request: g4Request(
          tasks.find((t) => t.task_id === c.task_id),
          c,
          root ?? "<unavailable>",
          suite.protocol,
        ),
      });
    await writeJson(join(output, "calls.json"), records);
    manifest.finished_at = new Date().toISOString();
    await writeJson(join(output, "run.json"), manifest);
  }
}

export async function collectReferenceIdentities(
  client,
  output,
  root,
  tasks,
  records,
  view,
) {
  const map = new Map(),
    results = [];
  for (const record of records.filter(
    (r) => r.family === "explore" && r.raw_path,
  )) {
    const parsed = parseGraphResponse(
      await readJson(join(output, record.raw_path)),
      "explore",
    );
    if (parsed.status === "success") {
      collectNodes(parsed, map);
      results.push({ record, parsed });
    }
  }
  const targets = new Set();
  for (const { record, parsed } of results) {
    const task = tasks.find((t) => t.task_id === record.task_id);
    for (const expected of task.truth.edges)
      for (const ref of parsed.graph.references) {
        if (
          ref.status === "resolved" &&
          ref.evidence.source_id === expected.evidence.path &&
          ref.evidence.span.start === expected.evidence.start &&
          !map.has(ref.target)
        )
          targets.add(ref.target);
      }
  }
  const reads = [];
  for (const target of [...targets].sort()) {
    const request = {
      name: "zvec_grep_explore",
      arguments: {
        root,
        request: {
          domain: "code",
          operation: {
            operation: "locate",
            target: {
              by: "node",
              node: { graph_build_id: view.graph.build_id, node_id: target },
            },
          },
        },
      },
    };
    const response = await client.request("tools/call", request);
    reads.push({
      request,
      response,
      response_sha256: objectHash(response),
      purpose:
        "Unscored public identity lookup for a resolved reference target; no body evidence credit.",
    });
  }
  await writeJson(join(output, "reference-identities.json"), reads);
}

export async function main(args = process.argv.slice(2)) {
  const { values } = parseArgs({
    args,
    options: {
      package: { type: "string" },
      output: { type: "string" },
      corpus: { type: "string" },
      "model-cache": { type: "string" },
      "source-cache": { type: "string" },
      "candidate-commit": { type: "string", default: "unrecorded" },
      binary: { type: "string" },
      "native-library-dir": { type: "string" },
    },
  });
  assert.ok(
    values.package && values.output && values.corpus && values["model-cache"],
    "G4 requires --package --output NEW_DIR --corpus NEW_DIR --model-cache",
  );
  const suite = await loadG4Suite();
  const options = {
    output: resolve(values.output),
    corpus: resolve(values.corpus),
    modelCache: resolve(values["model-cache"]),
    sourceCache: values["source-cache"]
      ? resolve(values["source-cache"])
      : null,
    candidateCommit: values["candidate-commit"],
  };
  await mkdir(options.output);
  await mkdir(options.corpus, { recursive: true });
  await writeJson(join(options.output, "suite.json"), {
    identity: suite.identity,
    protocol: suite.protocol,
    inputs: suite.inputs,
  });
  const candidate = await packageCandidate(values.package, options.output, {
    includeMcpClient: false,
  });
  const library =
    platform() === "darwin" ? "libzvec_c_api.dylib" : "libzvec_c_api.so";
  const packagedBinaryHash = await fileHash(candidate.cli);
  const packagedLibraryHash = await fileHash(
    join(candidate.packageRoot, "bin", library),
  );
  if (values.binary) {
    const binary = resolve(values.binary);
    assert.equal(
      await fileHash(binary),
      packagedBinaryHash,
      "--binary differs from packed payload",
    );
    candidate.cli = binary;
    candidate.identity.execution_path =
      "original-build-path-with-identical-packed-bytes";
    candidate.identity.packaged_binary_sha256 = packagedBinaryHash;
  }
  options.nativeLibraryDir = resolve(
    values["native-library-dir"] ?? join(candidate.packageRoot, "bin"),
  );
  assert.equal(
    await fileHash(join(options.nativeLibraryDir, library)),
    packagedLibraryHash,
    "runtime library differs from packed payload",
  );
  candidate.identity.runtime_library_sha256 = packagedLibraryHash;
  candidate.identity.binary_sha256 = await fileHash(candidate.cli);
  for (const repo of suite.inputs.repositories)
    await runRepo(suite, repo, candidate, options);
  const report = await aggregateG4(options.output);
  console.log(`G4 report: ${join(options.output, "report.md")}`);
  if (!report.integrity_passed) process.exitCode = 1;
}
