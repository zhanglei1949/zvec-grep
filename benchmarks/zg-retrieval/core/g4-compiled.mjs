import assert from "node:assert/strict";
import {
  copyFile,
  mkdir,
  readFile,
  readdir,
  writeFile,
} from "node:fs/promises";
import { dirname, join } from "node:path";
import { suiteDirectory } from "./suite.mjs";
import { fileHash, objectHash, readJson, run, sha256 } from "./io.mjs";
import { modelArtifactManifest, repositorySlug } from "./corpus.mjs";

export const COMPILED_PROFILE = "g4-compiled-v1";
export const COMPILED_REVISION = "g4-compiled-v2";
export const COMPILED_PROFILES = [COMPILED_PROFILE, COMPILED_REVISION];
export const compiledFixtures = join(suiteDirectory, "fixtures/g4-compiled");

export async function verifyCompiledFreeze(profile = COMPILED_PROFILE) {
  const freeze = await readJson(join(compiledFixtures, "freeze.json"));
  for (const entry of freeze.files)
    assert.equal(
      await fileHash(
        entry.path.startsWith("knowledge/archive/four-language-query-r0-r1/")
          ? join(compiledFixtures, entry.path.split("/").at(-1))
          : join(suiteDirectory, "../..", entry.path),
      ),
      entry.sha256,
      `frozen R1 file changed: ${entry.path}`,
    );
  if (profile === COMPILED_REVISION) {
    const revision = await readJson(
      join(compiledFixtures, "revision-freeze.json"),
    );
    for (const entry of revision.files)
      assert.equal(
        await fileHash(join(suiteDirectory, "../..", entry.path)),
        entry.sha256,
        `frozen revision changed: ${entry.path}`,
      );
  }
  return freeze;
}

export async function unpackCompiledSources(directory) {
  await verifyCompiledFreeze();
  await mkdir(directory);
  await run("tar", [
    "-xzf",
    join(compiledFixtures, "sources.tar.gz"),
    "-C",
    directory,
  ]);
  return join(directory, "sources");
}

export async function verifyCompiledModel(directory) {
  const handoff = await readJson(join(compiledFixtures, "handoff.json"));
  const current = await modelArtifactManifest(directory);
  const byPath = (entries) =>
    Object.fromEntries(entries.map((e) => [e.path, e.sha256]));
  assert.deepEqual(
    byPath(current.entries),
    byPath(handoff.model.files),
    "model differs from frozen R1 artifacts",
  );
  return current;
}

function selectedFiles(repo, boundary) {
  return boundary
    ? boundary.workspace_files
    : [...repo.selection.files, ...repo.selection.configuration_files];
}

export function compiledAnnotations(inputs, repository) {
  const calls = inputs.reference_annotations
    .filter(
      (a) =>
        a.repository === repository &&
        ["resolved", "unresolved", "declaration_only_or_unresolved"].includes(
          a.resolution_expectation,
        ),
    )
    .map((a) => {
      if (a.resolution_expectation !== "declaration_only_or_unresolved")
        return { ...a, kind: "calls" };
      const boundary = inputs.boundary_cases.find(
        (b) =>
          b.repository === repository &&
          b.expected === a.resolution_expectation &&
          b.evidence.some(
            (e) =>
              e.path === a.site.path &&
              e.start <= a.site.start &&
              a.site.end <= e.end,
          ),
      );
      assert.ok(
        boundary,
        "native declaration lacks independent frozen evidence",
      );
      const declaration = boundary.evidence[1];
      return {
        ...a,
        kind: "calls",
        target: {
          path: declaration.path,
          name: a.site.quote,
          span: { start: declaration.start, end: declaration.end },
        },
      };
    });
  const types = inputs.relation_annotations
    .filter((a) => a.repository === repository && a.target)
    .map((a) => {
      const context = a.evidence[0];
      const column = context.quote.lastIndexOf(a.target.name);
      assert.ok(
        column >= 0,
        "type relation target absent from frozen declaration",
      );
      const start =
        context.start + Buffer.byteLength(context.quote.slice(0, column));
      return {
        ...a,
        kind: a.relation,
        expression_context: context,
        site: {
          path: context.path,
          sha256: context.sha256,
          start,
          end: start + Buffer.byteLength(a.target.name),
        },
      };
    });
  return [...calls, ...types];
}

export function matchesCompiledReference(ref, annotation) {
  const site = annotation.site,
    context = annotation.expression_context;
  return (
    ref.kind === annotation.kind &&
    ref.evidence.source_id === site.path &&
    context.start <= ref.evidence.span.start &&
    ref.evidence.span.start <= site.start &&
    site.end <= ref.evidence.span.end &&
    ref.evidence.span.end <= context.end
  );
}

export async function materializeCompiledCorpus(
  repo,
  root,
  sources,
  boundary = null,
) {
  await mkdir(root);
  for (const path of selectedFiles(repo, boundary)) {
    const expected = repo.files.find((f) => f.path === path);
    assert.ok(expected, `source outside frozen selection: ${path}`);
    const source = join(sources, repositorySlug(repo.repository), path);
    assert.equal(
      await fileHash(source),
      expected.sha256,
      `source drift: ${path}`,
    );
    const target = join(root, path);
    await mkdir(dirname(target), { recursive: true });
    await copyFile(source, target);
  }
  const config = !boundary && repo.selection.generated_configuration;
  if (config) {
    const bytes = `${JSON.stringify(config.entries, null, 2)}\n`;
    assert.equal(
      sha256(bytes),
      config.sha256,
      "generated configuration changed",
    );
    await writeFile(join(root, config.path), bytes, { flag: "wx" });
  }
  await compiledCorpusManifest(root, repo, boundary);
  return root;
}

export async function compiledCorpusManifest(root, repo, boundary = null) {
  const expected = new Map(
    selectedFiles(repo, boundary).map((path) => {
      const file = repo.files.find((f) => f.path === path);
      assert.ok(file, `unfrozen source ${path}`);
      return [path, file];
    }),
  );
  const config = !boundary && repo.selection.generated_configuration;
  if (config) expected.set(config.path, config);
  const entries = [];
  async function visit(prefix = "") {
    for (const item of (
      await readdir(join(root, prefix), { withFileTypes: true })
    ).sort((a, b) => a.name.localeCompare(b.name))) {
      if (!prefix && item.name === ".zvec-grep") continue;
      const path = prefix ? `${prefix}/${item.name}` : item.name;
      if (item.isDirectory()) await visit(path);
      else {
        assert.ok(item.isFile(), `non-file in frozen corpus: ${path}`);
        assert.ok(expected.has(path), `unexpected indexed-tree file: ${path}`);
        const bytes = await readFile(join(root, path));
        const sha = sha256(bytes);
        assert.equal(sha, expected.get(path).sha256, `corpus changed: ${path}`);
        assert.equal(
          bytes.length,
          expected.get(path).bytes,
          `source size changed: ${path}`,
        );
        entries.push({ path, kind: "file", size: bytes.length, sha256: sha });
      }
    }
  }
  await visit();
  assert.equal(entries.length, expected.size, "frozen corpus files missing");
  return { sha256: objectHash(entries), entries };
}
