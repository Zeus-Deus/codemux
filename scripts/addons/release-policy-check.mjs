// Release-policy guard. The desktop updater, install.sh and hosted-client
// deploys read this repository's Latest release, and `v*` tags start the desktop
// Release workflow. Add-on and catalog releases must never claim either, and the
// desktop release must upload only installers that its add-on gate verified.
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import {
  chmod,
  mkdtemp,
  readdir,
  readFile,
  rm,
  writeFile,
} from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";

const workflow = (name) => readFile(`.github/workflows/${name}`, "utf8");
// The text of one job, up to the next key at the same or a lower indentation.
function job(text, name) {
  const lines = text.split("\n");
  const start = lines.indexOf(`  ${name}:`);
  assert.ok(start >= 0, `Missing job ${name}`);
  const end = lines.findIndex((line, i) => i > start && /^ {0,2}\S/.test(line));
  return lines.slice(start, end < 0 ? undefined : end).join("\n");
}
// The `run: |` script of one step in a job's text, without its indentation.
function stepScript(text, name) {
  const lines = text.split("\n");
  const start = lines.findIndex((line) => line.trim() === `- name: ${name}`);
  assert.ok(start >= 0, `Missing step ${name}`);
  const run = lines.findIndex(
    (line, i) => i > start && /^ +run: \|$/.test(line),
  );
  const indent = lines[run].indexOf("run:");
  const end = lines.findIndex(
    (line, i) => i > run && line.trim() && line.search(/\S/) <= indent,
  );
  return lines
    .slice(run + 1, end < 0 ? undefined : end)
    .map((line) => line.slice(indent + 2))
    .join("\n");
}
// Each `gh release create` command, including backslash continuation lines.
const releaseCommands = (text) =>
  text.match(/gh release create (?:[^\n]*\\\n)*[^\n]*/g) ?? [];

test("catalog publication needs an approved dispatch and keeps Latest", async () => {
  const publish = job(await workflow("addon-catalog.yml"), "publish");
  assert.match(
    publish,
    /^ {4}if: github\.event_name == 'workflow_dispatch' && github\.ref == 'refs\/heads\/main'$/m,
  );
  assert.match(publish, /^ {4}environment: addon-catalog$/m);
  assert.equal(releaseCommands(publish).length, 1);
  assert.match(publish, /releases\/latest" --jq \.tag_name/);
});

test(
  "catalog publication restores Latest only when it left the desktop",
  { skip: process.platform === "win32" },
  async () => {
    const script = stepScript(
      job(await workflow("addon-catalog.yml"), "publish"),
      "Publish immutable reviewed artifact",
    );
    // A stand-in for gh: Latest is one file, and creating the catalog release
    // lets the scenario move it the way GitHub or a desktop release could.
    const gh = `#!/usr/bin/env bash
set -euo pipefail
case "$1 $2" in
  "api repos/"*) cat "$STATE/latest" ;;
  "release view") exit 1 ;;
  "release create")
    case $SCENARIO in
      desktop) echo v0.23.0 > "$STATE/latest" ;;
      catalog) echo "$3" > "$STATE/latest" ;;
    esac ;;
  "release edit") [ "$4" = --latest ] && echo "$3" > "$STATE/latest" ;;
  *) echo "unexpected gh $*" >&2; exit 2 ;;
esac
`;
    for (const [scenario, code, latest] of [
      ["unchanged", 0, "v0.22.8"],
      // A desktop release published between the two reads keeps Latest.
      ["desktop", 0, "v0.23.0"],
      ["catalog", 1, "v0.22.8"],
    ]) {
      const dir = await mkdtemp(join(tmpdir(), "codemux-catalog-publish-"));
      try {
        await writeFile(join(dir, "gh"), gh);
        await chmod(join(dir, "gh"), 0o755);
        await writeFile(join(dir, "latest"), "v0.22.8\n");
        await writeFile(join(dir, "catalog-v1.json"), '{"revision":7}\n');
        const result = spawnSync(
          "bash",
          ["--noprofile", "--norc", "-eo", "pipefail", "-c", script],
          {
            cwd: dir,
            encoding: "utf8",
            env: {
              ...process.env,
              PATH: `${dir}:${process.env.PATH}`,
              STATE: dir,
              SCENARIO: scenario,
              GH_REPO: "example/codemux",
              GITHUB_SHA: "0".repeat(40),
              REQUESTED_REVISION: "7",
            },
          },
        );
        assert.equal(
          result.status,
          code,
          `${scenario}: ${result.stdout}${result.stderr}`,
        );
        assert.equal(
          (await readFile(join(dir, "latest"), "utf8")).trim(),
          latest,
          scenario,
        );
      } finally {
        await rm(dir, { recursive: true, force: true });
      }
    }
  },
);

test("add-on release commands never take Latest or a v* tag", async () => {
  const sources = [
    ["RELEASING.md", await readFile("docs/addons/RELEASING.md", "utf8")],
  ];
  for (const name of await readdir(".github/workflows"))
    sources.push([name, await workflow(name)]);
  let commands = 0;
  for (const [name, text] of sources)
    for (const command of releaseCommands(text)) {
      commands++;
      assert.match(command, /--latest=false/, `${name}: ${command}`);
      assert.doesNotMatch(command, /^gh release create ["']?v/, name);
    }
  assert.ok(commands >= 2, "Expected catalog and package release commands");
});

test("desktop releases upload only the bundles the add-on gate verified", async () => {
  const text = await workflow("release.yml");
  const actions = text.match(/uses: tauri-apps\/tauri-action@/g) ?? [];
  const verified =
    text.match(
      /^ +tauriScript: node scripts\/addons\/verified-tauri-build\.mjs$/gm,
    ) ?? [];
  assert.ok(actions.length > 0);
  assert.equal(verified.length, actions.length);
  // A separate build would verify different bytes from the ones published.
  assert.doesNotMatch(
    text,
    /^\s*(npm run tauri|npx tauri|cargo tauri)\b.*\bbuild\b/m,
  );
});
