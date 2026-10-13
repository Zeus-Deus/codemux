import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { readFile } from "node:fs/promises";
import { spawnSync } from "node:child_process";

// Bun executables must stay exact, unlike native ELF helpers whose RPATHs move.
// The dedicated probe explicitly performs zero auth/model turns.
export async function verifyManagedSidecar(staged, shipped, platform = process.platform) {
  const [original, packaged] = await Promise.all([readFile(staged), readFile(shipped)]);
  const hash = (bytes) => createHash("sha256").update(bytes).digest("hex");
  const sha256 = hash(original);
  assert.equal(hash(packaged), sha256, "Installer managed sidecar differs from the staged build");
  if (platform !== "linux") return { sha256, nativeProbe: null };
  const result = spawnSync(shipped, ["--check-native"], { encoding: "utf8", timeout: 30000 });
  assert.equal(result.status, 0, result.stderr || result.error?.message || "Installer native readiness failed");
  const nativeProbe = JSON.parse(result.stdout);
  assert.equal(nativeProbe.modelTurns, 0, "Native readiness must never make model turns");
  for (const field of ["ripgrepReady", "parserReady", "workspaceReady"]) assert.equal(nativeProbe[field], true, `Installer ${field} failed`);
  return { sha256, nativeProbe };
}
