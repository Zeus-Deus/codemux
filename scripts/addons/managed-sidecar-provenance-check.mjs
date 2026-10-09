import assert from "node:assert/strict";
import { chmod, copyFile, mkdtemp, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";
import { verifyManagedSidecar } from "./managed-sidecar-provenance.mjs";

async function fixture(callback) {
  const root = await mkdtemp(join(tmpdir(), "codemux sidecar-proof-"));
  const staged = join(root, "staged"), shipped = join(root, "shipped");
  const write = async (body) => {
    await writeFile(staged, `#!/bin/sh\n${body}\n`);
    await chmod(staged, 0o755); await copyFile(staged, shipped);
  };
  try { await callback({ staged, shipped, write }); } finally { await rm(root, { recursive: true, force: true }); }
}
const ready = { modelTurns: 0, ripgrepReady: true, parserReady: true, workspaceReady: true };
const linux = process.platform === "linux";

test("packaged native readiness executes only the explicit probe and records exact bytes", { skip: !linux }, async () => {
  await fixture(async ({ staged, shipped, write }) => {
    await write(`test "$#" = 1 && test "$1" = --check-native || exit 43\nprintf '%s\\n' '${JSON.stringify(ready)}'`);
    const result = await verifyManagedSidecar(staged, shipped, "linux");
    assert.match(result.sha256, /^[a-f0-9]{64}$/);
    assert.deepEqual(result.nativeProbe, ready);
  });
});

test("changed sidecar bytes fail before any executable is run", async () => {
  await fixture(async ({ staged, shipped, write }) => {
    await write("exit 44");
    await writeFile(shipped, "different artifact");
    await assert.rejects(verifyManagedSidecar(staged, shipped, "linux"), /differs from the staged build/);
  });
});

test("failed native load, missing evidence or a nonzero model-turn report fail packaged verification", { skip: !linux }, async () => {
  await fixture(async ({ staged, shipped, write }) => {
    await write("echo 'broken native helper' >&2; exit 45");
    await assert.rejects(verifyManagedSidecar(staged, shipped, "linux"), /broken native helper/);
    for (const evidence of [{ ...ready, modelTurns: 1 }, { ...ready, parserReady: false }, { modelTurns: 0 }]) {
      await write(`printf '%s\\n' '${JSON.stringify(evidence)}'`);
      await assert.rejects(verifyManagedSidecar(staged, shipped, "linux"));
    }
  });
});

test("Windows checks executable provenance without invoking a Linux-only native probe", async () => {
  await fixture(async ({ staged, shipped, write }) => {
    await write("exit 46");
    const result = await verifyManagedSidecar(staged, shipped, "win32");
    assert.equal(result.nativeProbe, null);
    assert.match(result.sha256, /^[a-f0-9]{64}$/);
  });
});
