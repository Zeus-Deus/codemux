// An author project installs the SDK tarball without its lockfile, and esbuild
// bundles the installed Remote DOM and Preact packages into plugin.js. Fail when
// any installed copy differs from the SDK lockfile, so release bytes depend on
// reviewed versions rather than on registry state at build time.
// Usage: node scripts/addons/sdk-lock.mjs <project directory> <SDK package-lock.json>
import assert from "node:assert/strict";
import { readdir, readFile } from "node:fs/promises";
import { join, resolve } from "node:path";
import { pathToFileURL } from "node:url";

// Every installed package, including nested copies, keyed by its path.
export async function installed(project) {
  const packages = [];
  async function visit(modules) {
    let entries;
    try {
      entries = await readdir(join(project, modules), { withFileTypes: true });
    } catch (error) {
      if (error.code === "ENOENT") return;
      throw error;
    }
    for (const entry of entries) {
      if (!entry.isDirectory() || entry.name.startsWith(".")) continue;
      if (entry.name.startsWith("@")) {
        await visit(join(modules, entry.name));
        continue;
      }
      const path = join(modules, entry.name);
      const { name, version } = JSON.parse(
        await readFile(join(project, path, "package.json"), "utf8"),
      );
      packages.push({ path: path.replaceAll("\\", "/"), name, version });
      await visit(join(path, "node_modules"));
    }
  }
  await visit("node_modules");
  return packages.sort((a, b) => a.path.localeCompare(b.path));
}

export async function verify(project, lockfile) {
  const lock = JSON.parse(await readFile(lockfile, "utf8"));
  const pinned = new Map();
  for (const [path, entry] of Object.entries(lock.packages))
    if (path && !entry.dev)
      pinned.set(
        path.slice(path.lastIndexOf("node_modules/") + 13),
        entry.version,
      );
  const tree = await installed(project);
  for (const { path, name, version } of tree)
    if (pinned.has(name))
      assert.equal(
        version,
        pinned.get(name),
        `${project}: ${path} is ${version}, but the SDK lockfile pins ${pinned.get(name)}`,
      );
  for (const name of pinned.keys())
    assert.ok(
      tree.some((p) => p.name === name),
      `${project}: ${name} from the SDK lockfile is not installed`,
    );
  return tree;
}

if (import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  const [project, lockfile] = process.argv.slice(2);
  await verify(project, lockfile);
  console.log(`${project}: SDK runtime dependencies match ${lockfile}`);
}
