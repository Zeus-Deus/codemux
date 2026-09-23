// Static release-policy guard. The desktop updater, install.sh and hosted-client
// deploys read this repository's Latest release, and `v*` tags start the desktop
// Release workflow. Add-on and catalog releases must never claim either, and the
// desktop release must upload only installers that its add-on gate verified.
import assert from "node:assert/strict";
import { readdir, readFile } from "node:fs/promises";
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
