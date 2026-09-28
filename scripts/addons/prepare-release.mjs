// Build reviewable author distributions from one committed source revision.
// This script never publishes, modifies the checkout, or builds the desktop.
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { cp, mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { verify } from "./sdk-lock.mjs";

const repository = resolve(dirname(fileURLToPath(import.meta.url)), "../..");
const output = resolve(process.argv[2] ?? "addon-author-release");
function run(command, args, cwd, capture = false) {
  const result = spawnSync(command, args, {
    cwd,
    encoding: "utf8",
    stdio: capture ? ["ignore", "pipe", "inherit"] : "inherit",
  });
  if (result.error) throw result.error;
  assert.equal(result.status, 0, `${command} failed`);
  return result.stdout?.trim();
}
const sourceCommit = run("git", ["rev-parse", "HEAD"], repository, true);
assert.match(sourceCommit, /^[a-f0-9]{40}$/);
assert.equal(
  run(
    "git",
    [
      "status",
      "--porcelain",
      "--",
      "packages/plugin-sdk",
      "packages/plugin-cli",
      "examples/addons",
    ],
    repository,
    true,
  ),
  "",
  "Commit author package changes before preparing release provenance",
);
// A release tag must reference a commit that stays on main. Other output, such
// as a pull request head or a local branch, is only a review candidate.
if (
  !run(
    "git",
    ["branch", "--remotes", "--contains", sourceCommit, "--list", "*/main"],
    repository,
    true,
  )
)
  console.warn(
    `Warning: ${sourceCommit} is not on a fetched remote main branch. Treat this output as a review candidate; prepare release assets from the exact main commit that will be tagged.`,
  );
// Refuse to replace an existing release directory or its assets.
await mkdir(output);
const temporary = await mkdtemp(join(tmpdir(), "codemux-author-release-"));
const assets = [];
const digest = (bytes) => createHash("sha256").update(bytes).digest("hex");
async function asset(path, metadata) {
  const bytes = await readFile(path);
  const name = path.split(/[\\/]/).at(-1);
  await cp(path, join(output, name), { errorOnExist: true, force: false });
  assets.push({
    name,
    sha256: digest(bytes),
    bytes: bytes.length,
    ...metadata,
  });
}
try {
  const archive = join(temporary, "source.tar");
  run(
    "git",
    [
      "archive",
      "--format=tar",
      `--output=${archive}`,
      sourceCommit,
      "packages/plugin-sdk",
      "packages/plugin-cli",
      "examples/addons",
    ],
    repository,
  );
  run("tar", ["-xf", archive, "-C", temporary], repository);
  const packed = join(temporary, "packed");
  await mkdir(packed);
  const tools = [];
  for (const name of ["plugin-sdk", "plugin-cli"]) {
    const directory = join(temporary, "packages", name);
    run(
      "npm",
      ["ci", "--ignore-scripts", "--no-audit", "--no-fund"],
      directory,
    );
    if (name === "plugin-sdk") run("npm", ["run", "build"], directory);
    const metadata = JSON.parse(
      await readFile(join(directory, "package.json")),
    );
    const [pack] = JSON.parse(
      run(
        "npm",
        ["pack", "--ignore-scripts", "--json", "--pack-destination", packed],
        directory,
        true,
      ),
    );
    const path = join(packed, pack.filename);
    tools.push(path);
    await asset(path, {
      kind: "author-tool",
      package: metadata.name,
      version: metadata.version,
    });
  }
  for (const name of ["project-brief", "issue-companion"]) {
    const directory = join(temporary, "examples/addons", name);
    // These directories are outside the app checkout. They use only the same
    // public tarballs available to third-party authors, with no workspace links.
    run(
      "npm",
      [
        "install",
        "--no-save",
        "--package-lock=false",
        "--ignore-scripts",
        "--no-audit",
        "--no-fund",
        ...tools,
      ],
      directory,
    );
    // Bundled runtime packages must equal the reviewed SDK lockfile; the
    // whole resolved author tree is recorded with the package.
    const tree = await verify(
      directory,
      join(temporary, "packages/plugin-sdk/package-lock.json"),
    );
    for (const command of ["build", "check", "pack"])
      run("npm", ["run", command], directory);
    const manifest = JSON.parse(
      await readFile(join(directory, "manifest.json")),
    );
    const path = join(directory, `${manifest.id}-${manifest.version}.cmxaddon`);
    const first = digest(await readFile(path));
    run("npm", ["run", "pack"], directory);
    assert.equal(
      digest(await readFile(path)),
      first,
      "Package bytes must be deterministic",
    );
    await asset(path, {
      kind: "feature-plugin",
      id: manifest.id,
      version: manifest.version,
      manifest,
      dependencies: Object.fromEntries(tree.map((p) => [p.path, p.version])),
    });
  }
  await writeFile(
    join(output, "provenance.json"),
    JSON.stringify({ schemaVersion: 1, sourceCommit, assets }, null, 2) + "\n",
    { flag: "wx" },
  );
  await writeFile(
    join(output, "SHA256SUMS"),
    assets.map((a) => `${a.sha256}  ${a.name}\n`).join(""),
    { flag: "wx" },
  );
  console.log(
    `Prepared ${assets.length} independent distributions from ${sourceCommit} in ${output}. Review and native acceptance are required before publication.`,
  );
} finally {
  // Only the newly-created temporary build directory is removed.
  await rm(temporary, { recursive: true, force: true });
}
