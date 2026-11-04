import { readdirSync, readFileSync, lstatSync } from "node:fs";
import { join, relative } from "node:path";
import { createHash } from "node:crypto";

const root = process.argv[2];
const hashes = {};
function visit(dir) {
  for (const name of readdirSync(dir).sort()) {
    const path = join(dir, name);
    const stat = lstatSync(path);
    if (stat.isSymbolicLink()) throw new Error(`index symlink: ${path}`);
    if (stat.isDirectory()) visit(path);
    else if (stat.isFile()) hashes[relative(root, path)] = createHash("sha256").update(readFileSync(path)).digest("hex");
    else throw new Error(`unexpected index entry: ${path}`);
  }
}
visit(root);
process.stdout.write(`${JSON.stringify(hashes)}\n`);
